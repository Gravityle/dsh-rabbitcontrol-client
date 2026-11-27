//! 被控端 clip 模式：把一份 Annex-B 码流当作"屏幕"发出去。
//!
//! 为什么需要它：真被控端（`rc-cli host --backend real`）要屏幕录制权限（TCC），没授
//! 权就起不来；假后端的帧又是合成数据、WebCodecs 解不了。这个模式用**假采集 + 直通
//! 编码器**把片段里的真 H.264 经同一条 QUIC 会话发出去，于是「设备列表 → 连接 → 真画面」
//! 这条链路不需要任何系统权限就能完整验证。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_encode::EncoderConfig;
use rc_pipeline::{DiffMode, HostPipeline, PipelineConfig};
use rc_session::proto::SessionParams;
use rc_session::testkit::RecordingInjector;
use rc_session::{AutoApprover, HostAuthConfig, HostSession, HostSessionConfig, SessionClock};
use rc_types::{FrameKind, Resolution};

const TOOL_PERMISSIONS: rc_session::proto::ToolPermissions = rc_session::proto::ToolPermissions {
    allow_file_transfer: false,
    allow_terminal: false,
    allow_port_map: false,
};

use crate::clip::{fake_capture, split_access_units, unit_kind, AnchoredCapture, ClipEncoder};

/// 一次 clip host 的参数。
pub struct ClipHostOptions {
    /// 监听地址（QUIC）。
    pub listen: SocketAddr,
    /// Annex-B H.264 文件。
    pub clip: PathBuf,
    /// 声明的分辨率（也用于能力描述）。
    pub resolution: Resolution,
    /// 帧率。
    pub fps: u32,
    /// 声明码率。
    pub bitrate_bps: u32,
    /// accept 后服务多久。
    pub duration: Duration,
}

/// 跑一次 clip host：accept 一条连接，按帧率把片段发完（循环）后收尾。
///
/// @param options - 监听地址、片段、分辨率/帧率/时长。
pub fn run(options: &ClipHostOptions) -> Result<(), String> {
    let bytes = std::fs::read(&options.clip)
        .map_err(|e| format!("无法读片段 {}: {e}", options.clip.display()))?;
    let units = split_access_units(&bytes);
    if units.is_empty() {
        return Err(format!("{} 里没有解析出访问单元", options.clip.display()));
    }
    let keyframes = units
        .iter()
        .filter(|unit| unit_kind(unit) == FrameKind::Idr)
        .count();
    if keyframes == 0 {
        return Err("片段里没有关键帧，控制端无法开始解码".into());
    }
    eprintln!(
        "rc-bridge clip host：{} 个访问单元（关键帧 {keyframes}），{}x{} @ {}fps",
        units.len(),
        options.resolution.width,
        options.resolution.height,
        options.fps
    );

    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution: options.resolution,
        fps: options.fps,
        bitrate_bps: options.bitrate_bps,
        // 真实屏幕才需要硬件后端；片段回放用假后端即可（也更省电）。
        prefer_hardware: false,
        target_latency_us: 22_000,
        trace_capacity: 4096,
        diff: DiffMode::TrustPlatform,
        clock_origin_us: None,
        max_packet_payload: rc_pipeline::MAX_PAYLOAD,
        ..PipelineConfig::default()
    };

    let cfg = HostSessionConfig {
        listen_addr: options.listen,
        mtu: rc_session::DEFAULT_SESSION_MTU,
        caps: SessionParams {
            width: options.resolution.width,
            height: options.resolution.height,
            fps: options.fps,
            bitrate_bps: options.bitrate_bps,
        },
        pipeline: pipeline.clone(),
        handshake_timeout: Duration::from_secs(10),
        // 纯监听（回放片段）不注入真实输入：只喂测试画面时不该动本机的鼠标键盘。
        injector: Box::new(RecordingInjector::new()),
        // 假采集必须节流。
        pace_capture: true,
        auth: HostAuthConfig::default(),
        tool_permissions: TOOL_PERMISSIONS,
        approver: Box::new(AutoApprover),
    };

    let start = Instant::now();
    let clock: SessionClock = {
        let start = start;
        Arc::new(move || u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX))
    };

    // 管线工厂：假采集（只按帧率触发）+ 直通编码器（交出片段里的真码流）。
    let factory = {
        let bytes = bytes.clone();
        Box::new(move |pcfg: &PipelineConfig| -> rc_types::Result<HostPipeline> {
            let encoder = ClipEncoder::new(
                &bytes,
                &EncoderConfig {
                    resolution: pcfg.resolution,
                    fps: pcfg.fps,
                    bitrate_bps: pcfg.bitrate_bps,
                    ..EncoderConfig::default()
                },
            )?;
            let capture = fake_capture(pcfg.resolution, pcfg.fps)?;
            HostPipeline::with_backends_paced(
                pcfg.clone(),
                Box::new(AnchoredCapture::new(capture, start)),
                Box::new(encoder),
                true,
            )
        })
    };

    let mut host = HostSession::new(
        cfg,
        rc_session::dev_cert::DEV_CERT_PEM,
        rc_session::dev_cert::DEV_KEY_PEM,
        clock,
    )
    .map_err(|e| format!("clip host 会话构造失败: {e}"))?;
    host.set_pipeline_factory_for(factory);

    let listen = host.local_addr().map_err(|e| format!("无法取监听地址: {e}"))?;
    eprintln!("rc-bridge clip host 等待连接：listen={listen}（accept 上限 300s）");

    host.accept_and_handshake(Duration::from_secs(300))
        .map_err(|e| format!("accept/握手失败: {e}"))?;
    let negotiated = host.negotiated().ok_or("握手完成却没有裁定值")?;
    eprintln!(
        "rc-bridge clip host 已连接：裁定 {}x{} @ {}fps @ {}bps，session_id={}",
        negotiated.width,
        negotiated.height,
        negotiated.fps,
        negotiated.bitrate_bps,
        host.session_id().unwrap_or(0)
    );

    let connected = Instant::now();
    let summary = host
        .run(&mut || connected.elapsed() >= options.duration)
        .map_err(|e| format!("clip host 会话运行失败: {e}"))?;
    eprintln!(
        "rc-bridge clip host 结束：采集 {} / 编码 {} 帧 / 数据报 {} / 注入 {} / 连接 {}ms",
        summary.captured,
        summary.frames_encoded,
        summary.datagrams_sent,
        summary.events_injected,
        connected.elapsed().as_millis()
    );
    Ok(())
}

/// 信令被控端的参数。
pub struct SignalHostOptions {
    /// 「屏幕」内容来源。
    pub clip: PathBuf,
    /// 声明的分辨率。
    pub resolution: Resolution,
    /// 帧率。
    pub fps: u32,
    /// 声明码率。
    pub bitrate_bps: u32,
    /// 每次会话跑多久。
    pub duration: Duration,
    /// rc-server 的 REST 地址。
    pub server_url: String,
    /// 账号 access token。
    pub token: String,
    /// 本端设备 key。
    pub device_key: String,
    /// 校验服务端 TLS 的 CA PEM。
    pub ca_pem_path: Option<String>,
    /// 上报给服务端的局域网候选地址。
    pub lan_addr: String,
    /// 设备密钥/证书目录。
    pub cert_dir: PathBuf,
    /// 等登记完成的上限。
    pub wait: Duration,
    /// 总运行时长（等连接 + 服务会话）。
    pub overall: Duration,
    /// 传输偏好（打洞 / 中继 / 自动）。
    pub transport: crate::TransportChoice,
    /// 内容来源：片段（彩条，跨平台）或真实屏幕（仅 macOS + `real-capture` 特性）。
    pub source: HostSource,
    /// 显式信令端口（None = 由 REST 端口推导为 api-2）。
    pub signal_port: Option<u16>,
}

/// 被控端的内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSource {
    /// 回放片段（平台无关，用于链路验证）。
    Clip,
    /// 真实屏幕：ScreenCaptureKit + VideoToolbox（仅 macOS 且带 `real-capture` 特性）。
    Real,
}

/// 信令被控端：登记上线 → 签发设备证书 → 等连接请求 → 走中继把片段发出去。
///
/// 中继是跨网段那条路（不需要打洞）。服务端仲裁后会给两端各下发一次性令牌，
/// 本端以 `Host` 角色连上中继，控制端以 `Client` 角色连上，中继把两端配对。
///
/// @param options - 片段、服务器、令牌、证书目录与时长。
pub fn run_signaling_host(options: &SignalHostOptions) -> Result<(), String> {
    use rc_client_core::relay_transport::SessionMtlsBundle;

    use crate::identity;
    use crate::signal;

    let bytes = match options.source {
        HostSource::Clip => {
            let bytes = std::fs::read(&options.clip)
                .map_err(|e| format!("无法读片段 {}: {e}", options.clip.display()))?;
            if split_access_units(&bytes).is_empty() {
                return Err(format!("{} 里没有解析出访问单元", options.clip.display()));
            }
            Some(bytes)
        }
        HostSource::Real => {
            register_real_backends()?;
            // 启动即探测输入注入权限：缺「辅助功能」时立刻说清楚，别等用户拖鼠标才发现。
            let (probe, real) = real_injector();
            drop(probe);
            eprintln!(
                "rc-bridge 信令被控端：输入注入={}",
                if real { "真实（CGEvent，可操作本机）" } else { "记录型（缺辅助功能权限，只能看不能操作）" }
            );
            eprintln!(
                "rc-bridge 信令被控端：内容来源=真实屏幕（{}x{} @ {}fps）",
                options.resolution.width, options.resolution.height, options.fps
            );
            None
        }
    };

    let cfg = signal::server_config(
        &options.server_url,
        &options.device_key,
        options.ca_pem_path.as_deref(),
        options.signal_port,
    )?;
    let manager = rc_client_core::cert_manager::CertManager::load_or_init(options.cert_dir.clone())?;
    let pubkey = manager.device_pubkey_hex();
    let (handle, device_id) = signal::register_and_wait(
        &cfg,
        &options.token,
        Some(pubkey),
        vec![options.lan_addr.clone()],
        options.wait,
        true, // The local host toggle starts/stops this process.
    )?;
    let identity = identity::provision(&cfg, &options.token, &device_id, &options.cert_dir)?;
    eprintln!(
        "rc-bridge 信令被控端：设备码 {} 在线，等待连接请求（中继路径）",
        identity.device_id
    );

    let verifier = identity::owner_verifier(&cfg, &identity)?;
    let auth = HostAuthConfig {
        allow_owner_control: true,
        allow_remote_assist: false,
        owner_verifier: Some(Arc::new(verifier)),
        replay_cache: Some(Arc::default()),
        ..HostAuthConfig::default()
    };
    let deadline = Instant::now() + options.overall;
    let mut served = 0u32;
    while Instant::now() < deadline {
        let Some(incoming) = handle.try_recv_incoming() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        eprintln!(
            "rc-bridge 信令被控端：收到来自 {} 的连接请求（session={}，中继 {}）",
            incoming.peer_id, incoming.session_id, incoming.relay_addr
        );
        if !incoming.is_owner_connection {
            eprintln!("rc-bridge：本机未开放跨账号协助");
            continue;
        }
        let mtls = SessionMtlsBundle {
            ca_cert_pem: identity.ca_pem.clone(),
            cert_pem: identity.cert_pem.clone(),
            key_pem: identity.key_pem.clone(),
            local_user_id: identity.user_id.clone(),
            expected_peer_device_id: Some(incoming.peer_id.clone()),
            expected_peer_user_id: Some(identity.user_id.clone()),
        };
        let transport = match choose_host_transport(options.transport, &incoming, &mtls) {
            Ok(transport) => transport,
            Err(error) => {
                eprintln!("rc-bridge 信令被控端：建链失败（继续等待下一次请求）：{error}");
                continue;
            }
        };
        match serve_over(options, bytes.as_deref(), transport, auth.clone()) {
            Ok(()) => {
                served += 1;
                eprintln!("rc-bridge 信令被控端：第 {served} 次会话结束，继续等下一个连接");
            }
            Err(error) => eprintln!("rc-bridge 信令被控端：会话失败（继续等）：{error}"),
        }
    }
    if served == 0 {
        return Err("整个等待期内没有服务过任何会话".into());
    }
    Ok(())
}

/// 按偏好选一条被控端传输：打洞优先，失败退回中继。
fn choose_host_transport(
    choice: crate::TransportChoice,
    incoming: &rc_client_core::signal_client::RelayIncoming,
    mtls: &rc_client_core::relay_transport::SessionMtlsBundle,
) -> Result<Box<dyn rc_net::Transport>, String> {
    use crate::p2p;
    use crate::relay;
    use rc_client_core::p2p::P2pCandidates;
    use rc_client_core::relay_transport::RelayRole;

    let to_relay = |label: &str| -> Result<Box<dyn rc_net::Transport>, String> {
        eprintln!("rc-bridge 选路：{label} → 中继 {}", incoming.relay_addr);
        relay::connect_relay(
            &incoming.relay_addr,
            RelayRole::Host,
            &incoming.relay_token,
            mtls,
            // 给对端留出"打洞失败再转过来"的时间，别让先到的一端等到超时。
            Duration::from_secs(20),
        )
    };

    if choice == crate::TransportChoice::Relay {
        return to_relay("按要求只用中继");
    }
    let candidates = P2pCandidates {
        session_id: incoming.session_id.clone(),
        peer_public_addr: incoming.peer_public_addr.clone(),
        peer_lan_addrs: incoming.peer_lan_addrs.clone(),
        local_udp_port: incoming.local_udp_port,
    };
    match p2p::punch_host(&candidates, mtls, &p2p::PunchBudget::default()) {
        Ok(transport) => Ok(transport),
        Err(error) if choice == crate::TransportChoice::P2p => {
            Err(format!("按要求只用打洞，但打洞失败：{error}"))
        }
        Err(error) => to_relay(&format!("打洞失败（{error}）")),
    }
}

/// 在一条已建立的传输上跑一次 clip 会话。
/// 真实输入注入器：把对端发来的鼠标键盘事件真的敲进本机（CGEvent）。
///
/// 构造时会用 `AXIsProcessTrusted()` 预检：没有「辅助功能」权限就拿不到，
/// 这时**降级为记录型**并打印明确原因——射流照旧，但拖不动鼠标，用户不会莫名其妙。
fn real_injector() -> (Box<dyn rc_input::InputInjector>, bool) {
    #[cfg(all(target_os = "macos", feature = "real-capture"))]
    {
        match rc_input_macos::InputBridge::new() {
            Ok(bridge) => (Box::new(bridge), true),
            Err(error) => {
                eprintln!(
                    "rc-bridge 输入注入不可用（{error}）：请在「系统设置 → 隐私与安全性 → 辅助功能」里勾上 DeepSeek Harness，然后重启 DSH。此会话只能看、不能操作。"
                );
                (Box::new(rc_session::testkit::RecordingInjector::new()), false)
            }
        }
    }
    #[cfg(not(all(target_os = "macos", feature = "real-capture")))]
    {
        eprintln!("rc-bridge 输入注入不可用：这个构建没有 real-capture 特性（仅 macOS 支持真实注入）");
        (Box::new(rc_session::testkit::RecordingInjector::new()), false)
    }
}

/// 真实屏幕后端的注册（把 macOS 的采集/编码装进 core 的平台槽位）。
///
/// 只在 macOS + `real-capture` 特性下可用；其它情况返回可读错误，而不是静默降级成假画面。
fn register_real_backends() -> Result<(), String> {
    #[cfg(all(target_os = "macos", feature = "real-capture"))]
    {
        rc_platform_macos::register_all();
        Ok(())
    }
    #[cfg(not(all(target_os = "macos", feature = "real-capture")))]
    {
        Err("这个构建不支持真实采集：需要 macOS 且以 `--features real-capture` 构建".into())
    }
}

fn serve_over(
    options: &SignalHostOptions,
    bytes: Option<&[u8]>,
    transport: Box<dyn rc_net::Transport>,
    auth: HostAuthConfig,
) -> Result<(), String> {
    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution: options.resolution,
        fps: options.fps,
        bitrate_bps: options.bitrate_bps,
        // 真实屏幕才需要硬件后端；片段回放用假后端即可（也更省电）。
        prefer_hardware: options.source == HostSource::Real,
        target_latency_us: 22_000,
        trace_capacity: 4096,
        diff: DiffMode::TrustPlatform,
        clock_origin_us: None,
        max_packet_payload: rc_pipeline::MAX_PAYLOAD,
        ..PipelineConfig::default()
    };
    let cfg = HostSessionConfig {
        // 传输已注入，地址不会被使用。
        listen_addr: "127.0.0.1:0".parse().expect("占位地址"),
        mtu: rc_session::DEFAULT_SESSION_MTU,
        caps: SessionParams {
            width: options.resolution.width,
            height: options.resolution.height,
            fps: options.fps,
            bitrate_bps: options.bitrate_bps,
        },
        pipeline: pipeline.clone(),
        handshake_timeout: Duration::from_secs(15),
        injector: if options.source == HostSource::Real {
            let (injector, real) = real_injector();
            eprintln!(
                "rc-bridge 信令被控端：输入注入={}",
                if real { "真实（CGEvent，可操作本机）" } else { "记录型（缺权限，不能操作）" }
            );
            injector
        } else {
            Box::new(RecordingInjector::new())
        },
        pace_capture: true,
        auth,
        tool_permissions: TOOL_PERMISSIONS,
        approver: Box::new(AutoApprover),
    };
    let start = Instant::now();
    let clock: SessionClock = {
        let start = start;
        Arc::new(move || u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX))
    };
    let (capture, encoder, backend_label): (Box<dyn rc_capture::CaptureBackend>, Box<dyn rc_encode::Encoder>, String) =
        match options.source {
            HostSource::Clip => {
                let bytes = bytes.ok_or("片段来源却没有片段数据")?;
                let encoder = ClipEncoder::new(
                    bytes,
                    &EncoderConfig {
                        resolution: options.resolution,
                        fps: options.fps,
                        bitrate_bps: options.bitrate_bps,
                        ..EncoderConfig::default()
                    },
                )
                .map_err(|e| format!("直通编码器构造失败：{e}"))?;
                let capture = fake_capture(options.resolution, options.fps)
                    .map_err(|e| format!("假采集构造失败：{e}"))?;
                (capture, Box::new(encoder), "clip+fake".to_string())
            }
            HostSource::Real => {
                let (capture, capture_label) = rc_pipeline::select_capture_labeled(&pipeline)
                    .map_err(|e| format!("采集后端选择失败：{e}"))?;
                let (encoder, encoder_label) = rc_pipeline::select_encoder_labeled(&pipeline)
                    .map_err(|e| format!("编码后端选择失败：{e}"))?;
                (capture, encoder, format!("{capture_label}+{encoder_label}"))
            }
        };
    eprintln!("rc-bridge 信令被控端：后端={backend_label}");
    let mut host = HostSession::with_parts(
        cfg,
        transport,
        Box::new(AnchoredCapture::new(capture, start)),
        encoder,
        clock,
    )
    .map_err(|e| format!("被控端会话构造失败：{e}"))?;
    let negotiated = host.handshake().map_err(|e| format!("握手失败：{e}"))?;
    eprintln!(
        "rc-bridge 信令被控端：已握手 {}x{} @ {}fps，开始发流",
        negotiated.width, negotiated.height, negotiated.fps
    );
    let connected = Instant::now();
    let summary = host
        .run(&mut || connected.elapsed() >= options.duration)
        .map_err(|e| format!("会话运行失败：{e}"))?;
    eprintln!(
        "rc-bridge 信令被控端：会话结束，采集 {} / 编码 {} 帧 / 数据报 {}",
        summary.captured, summary.frames_encoded, summary.datagrams_sent
    );
    Ok(())
}
