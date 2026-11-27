//! 被控端 clip 模式：把一份 Annex-B 码流当作"屏幕"发出去。
//!
//! 为什么需要它：真被控端（`rc-cli host --backend real`）要屏幕录制权限（TCC），没授
//! 权就起不来；假后端的帧又是合成数据、WebCodecs 解不了。这个模式用**假采集 + 直通
//! 编码器**把片段里的真 H.264 经同一条 QUIC 会话发出去，于是「设备列表 → 连接 → 真画面」
//! 这条链路不需要任何系统权限就能完整验证。

use std::collections::VecDeque;
use std::net::SocketAddr;
use rc_client_core::signal_client::RelayIncoming;
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

/// Clip bytes are passed through unchanged. Reject a Welcome that would mislabel them.
fn clip_resolution(bytes: &[u8], requested: Resolution) -> Result<Resolution, String> {
    let nals = crate::clip::nal_units(bytes);
    let mut resolution = None;
    for (i, &(_, header)) in nals.iter().enumerate() {
        if bytes[header] & 31 != 7 { continue; }
        let end = nals.get(i + 1).map_or(bytes.len(), |&(start, _)| start);
        let (w, h) = crate::sps::sps_dimensions(&bytes[header..end]).ok_or("无效的片段 SPS")?;
        let size = Resolution::new(w, h);
        if w < 2 || h < 2 || size.to_even() != size || resolution.is_some_and(|prev| prev != size) {
            return Err("片段需要固定的偶数像素尺寸".into());
        }
        resolution = Some(size);
    }
    let size = resolution.ok_or("片段缺少 H.264 SPS")?;
    if requested.width != 0 && (requested.width < size.width || requested.height < size.height) {
        return Err(format!("片段固有尺寸 {}x{} 超出请求上限", size.width, size.height));
    }
    Ok(size)
}

fn clip_pipeline(bytes: &[u8], size: Resolution, cfg: &PipelineConfig, start: Instant) -> rc_types::Result<HostPipeline> {
    if cfg.resolution != size {
        return Err(rc_types::Error::InvalidConfig(format!("片段固有尺寸 {}x{}，不支持缩放", size.width, size.height)));
    }
    let encoder = ClipEncoder::new(bytes, &rc_pipeline::default_encoder_config(cfg))?;
    // The encoder only needs a paced event, never a screen-sized pixel buffer.
    let capture = fake_capture(Resolution::new(2, 2), cfg.fps)?;
    HostPipeline::with_backends_paced(cfg.clone(), Box::new(AnchoredCapture::new(capture, start)), Box::new(encoder), true)
}

fn display_cap(native: Resolution, request: Resolution) -> Resolution {
    if request.width == 0 || (request.width >= native.width && request.height >= native.height) { return native; }
    let scale = (f64::from(request.width) / f64::from(native.width)).min(f64::from(request.height) / f64::from(native.height));
    let fit = |v: u32, limit: u32| (((f64::from(v) * scale / 2.0).round() as u32 * 2).min(limit & !1)).max(2);
    Resolution::new(fit(native.width, request.width), fit(native.height, request.height))
}

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
    let resolution = clip_resolution(&bytes, options.resolution)?;
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
        resolution.width,
        resolution.height,
        options.fps
    );

    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution,
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
            width: resolution.width,
            height: resolution.height,
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
    let factory = Box::new(move |cfg: &PipelineConfig| clip_pipeline(&bytes, resolution, cfg, start));

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
            Some(Arc::<[u8]>::from(bytes))
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
                "rc-bridge 信令被控端：内容来源=真实屏幕（{}fps，握手时读取主显示器像素）",
                options.fps
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
        signal::Registration::Host, // The local host toggle starts/stops this process.
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
    let mut pending = VecDeque::new();
    let mut recent = VecDeque::new();
    while Instant::now() < deadline {
        collect_requests(&mut pending, || handle.try_recv_incoming(), &recent);
        let Some(incoming) = pending.pop_front() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        eprintln!(
            "rc-bridge 信令被控端：收到来自 {} 的连接请求（session={}，中继 {}）",
            incoming.peer_id, incoming.session_id, incoming.relay_addr
        );
        if recent.len() == REQUEST_CAP { recent.pop_front(); }
        recent.push_back((incoming.peer_id.clone(), incoming.session_id.clone()));
        let mtls = SessionMtlsBundle {
            ca_cert_pem: identity.ca_pem.clone(),
            cert_pem: identity.cert_pem.clone(),
            key_pem: identity.key_pem.clone(),
            local_user_id: identity.user_id.clone(),
            expected_peer_device_id: Some(incoming.peer_id.clone()),
            expected_peer_user_id: Some(identity.user_id.clone()),
        };
        let (transport, active_switch) = match choose_host_transport(options.transport, &incoming, &mtls) {
            Ok(transport) => transport,
            Err(error) => {
                eprintln!("rc-bridge 信令被控端：建链失败（继续等待下一次请求）：{error}");
                continue;
            }
        };
        let mut poll_at = Instant::now();
        let mut stop = || {
            let now = Instant::now();
            if now < poll_at { return false; }
            poll_at = now + Duration::from_millis(25);
            collect_requests(&mut pending, || handle.try_recv_incoming(), &recent);
            pending.iter().any(|next| next.peer_id == incoming.peer_id)
        };
        match serve_over(options, bytes.clone(), transport, active_switch, auth.clone(), &mut stop) {
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

// Match the upstream signaling queue bound. Each grant is usable once.
const REQUEST_CAP: usize = 64;

fn collect_requests(
    pending: &mut VecDeque<RelayIncoming>,
    mut receive: impl FnMut() -> Option<RelayIncoming>,
    recent: &VecDeque<(String, String)>,
) {
    for _ in 0..REQUEST_CAP {
        let Some(next) = receive() else { break };
        if !next.is_owner_connection || recent.iter().any(|(peer, id)| {
            *peer == next.peer_id && *id == next.session_id
        }) { continue; }
        // Keep only the newest grant per device, preserving other peers' place.
        if let Some(slot) = pending.iter_mut().find(|old| old.peer_id == next.peer_id) {
            *slot = next;
        } else {
            if pending.len() == REQUEST_CAP { pending.pop_front(); }
            pending.push_back(next);
        }
    }
}

/// 按偏好选一条被控端传输：打洞优先，失败退回中继。
fn choose_host_transport(
    choice: crate::TransportChoice,
    incoming: &rc_client_core::signal_client::RelayIncoming,
    mtls: &rc_client_core::relay_transport::SessionMtlsBundle,
) -> Result<(Box<dyn rc_net::Transport>, Option<crate::auto_transport::ActiveSwitch>), String> {
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

    if incoming.is_relay_alloc && choice == crate::TransportChoice::P2p {
        return Err("对端要求中继，本端要求 P2P".into());
    }
    if incoming.is_relay_alloc || choice == crate::TransportChoice::Relay {
        eprintln!("rc-bridge route=relay");
        return to_relay("按要求只用中继").map(|t| (t, None));
    }
    let candidates = P2pCandidates {
        session_id: incoming.session_id.clone(),
        peer_public_addr: incoming.peer_public_addr.clone(),
        peer_lan_addrs: incoming.peer_lan_addrs.clone(),
        local_udp_port: incoming.local_udp_port,
        peer_public_addrs: incoming.peer_public_addrs.clone(),
        self_nat_type: incoming.self_nat_type,
        self_nat_port_delta: incoming.self_nat_port_delta,
        peer_nat_type: incoming.peer_nat_type,
        peer_nat_port_delta: incoming.peer_nat_port_delta,
        self_nat_port_range: incoming.self_nat_port_range,
        peer_nat_port_range: incoming.peer_nat_port_range,
        punch_secret: incoming.punch_secret.clone(),
        start_after_ms: incoming.start_after_ms,
        relay_fallback_after_ms: incoming.relay_fallback_after_ms,
    };
    let candidates = p2p::filter_candidates(candidates, incoming.self_public_addr.as_deref());
    if choice == crate::TransportChoice::Auto {
        return crate::auto_transport::connect(candidates, mtls.clone(), &incoming.relay_addr, &incoming.relay_token, RelayRole::Host);
    }
    match p2p::punch_host(&candidates, mtls, &p2p::PunchBudget::default()) {
        Ok(transport) => { eprintln!("rc-bridge route=p2p"); Ok((transport, None)) },
        Err(error) if choice == crate::TransportChoice::P2p => {
            Err(format!("按要求只用打洞，但打洞失败：{error}"))
        }
        Err(error) => to_relay(&format!("打洞失败（{error}）")).map(|t| (t, None)),
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
    bytes: Option<Arc<[u8]>>,
    transport: Box<dyn rc_net::Transport>,
    active_switch: Option<crate::auto_transport::ActiveSwitch>,
    auth: HostAuthConfig,
    stop: &mut dyn FnMut() -> bool,
) -> Result<(), String> {
    let resolution = match options.source {
        HostSource::Clip => clip_resolution(bytes.as_deref().ok_or("缺少片段数据")?, options.resolution)?,
        HostSource::Real => {
            let native = rc_client_core::platform::main_display_backing_resolution().ok_or("读取主显示器物理像素失败")?;
            let cap = display_cap(native, options.resolution);
            eprintln!("rc-bridge 主显示器 {}x{} backing pixels，会话上限 {}x{}", native.width, native.height, cap.width, cap.height);
            cap
        }
    };
    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution,
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
            width: resolution.width,
            height: resolution.height,
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
    // with_parts requires two backends even when a negotiated factory is installed.
    // Its seed backends are never started, and are independent of the display size.
    let seed = EncoderConfig { resolution: Resolution::new(2, 2), ..EncoderConfig::default() };
    let mut host = HostSession::with_parts(cfg, transport,
        fake_capture(seed.resolution, 1).map_err(|e| e.to_string())?,
        Box::new(rc_encode::FakeEncoder::new(seed).map_err(|e| e.to_string())?), clock)
        .map_err(|e| format!("被控端会话构造失败：{e}"))?;
    host.set_preserve_caps_aspect(true);
    host.set_pipeline_factory_for(Box::new(move |cfg| {
        if let Some(bytes) = &bytes { return clip_pipeline(bytes, resolution, cfg, start); }
        let (capture, cap_label) = rc_pipeline::select_capture_labeled(cfg)?;
        if cap_label != "sck" { return Err(rc_types::Error::InvalidConfig("ScreenCaptureKit 采集不可用".into())); }
        let (encoder, enc_label) = rc_pipeline::select_encoder_labeled(cfg)?;
        if enc_label != "videotoolbox" { return Err(rc_types::Error::InvalidConfig("VideoToolbox 编码不可用".into())); }
        eprintln!("rc-bridge 后端={cap_label}+{enc_label} {}x{} @ {}fps", cfg.resolution.width, cfg.resolution.height, cfg.fps);
        HostPipeline::with_backends_paced(cfg.clone(), Box::new(AnchoredCapture::new(capture, start)), encoder, true)
    }));
    let negotiated = host.handshake().map_err(|e| format!("握手失败：{e}"))?;
    eprintln!(
        "rc-bridge 信令被控端：已握手 {}x{} @ {}fps，开始发流",
        negotiated.width, negotiated.height, negotiated.fps
    );
    let connected = Instant::now();
    // This transport was injected by signaling. Detached has no listening
    // endpoint and can never resume here; the next grant creates a new host.
    let mut budget = crate::RUN_POLL_BUDGET;
    while host.state() == rc_session::SessionState::Streaming
        && connected.elapsed() < options.duration && !stop()
    {
        if let Some(switch) = &active_switch { switch.report(); }
        let report = host.tick(budget).map_err(|e| format!("会话运行失败：{e}"))?;
        if report.closed.is_some() { break; }
        budget = if report.busy { crate::RUN_POLL_BUDGET } else { crate::RUN_IDLE_POLL_BUDGET };
    }
    let summary = host.close(rc_session::proto::ByeReason::Normal).map_err(|e| format!("会话收尾失败：{e}"))?;
    eprintln!("rc-bridge 信令被控端：会话结束，采集 {} / 编码 {} 帧 / 数据报 {} / 注入 {}",
        summary.captured, summary.frames_encoded, summary.datagrams_sent, summary.events_injected);
    Ok(())
}

#[cfg(test)]
mod resolution_tests {
    use super::*;

    #[test]
    fn coalesces_owner_requests_without_reusing_grants() {
        let req = |peer: &str, id: &str, owner| RelayIncoming {
            peer_id: peer.into(), session_id: id.into(), is_owner_connection: owner,
            ..Default::default()
        };
        let mut stream = vec![req("a", "old", true), req("a", "fresh", true),
            req("b", "other", true), req("a", "latest", true), req("c", "guest", false),
            req("a", "old", true)].into_iter();
        let mut pending = VecDeque::new();
        collect_requests(&mut pending, || stream.next(), &VecDeque::from([("a".into(), "old".into())]));
        assert_eq!(pending, [req("a", "latest", true), req("b", "other", true)]);
    }

    #[test]
    fn auto_and_bounded_manual_requests() {
        let native = Resolution::new(3024, 1964);
        let auto = crate::parse_resolution("auto").unwrap();
        assert_eq!(auto, Resolution::new(0, 0));
        assert_eq!(display_cap(native, auto), native);
        assert_eq!(display_cap(native, crate::parse_resolution("1280x720").unwrap()), Resolution::new(1108, 720));
        assert_eq!(display_cap(native, Resolution::new(8000, 8000)), native);
        for invalid in ["0x0", "1x500", "16385x4096", "x1080"] {
            assert!(crate::parse_resolution(invalid).is_err());
        }
    }

    #[test]
    fn clip_uses_cropped_sps_and_rejects_undersized_request() {
        // 640x368 coded picture with 8 cropped lines, from testsrc.h264.
        let sps = [0,0,1,0x67,0x42,0xc0,0x1e,0xd9,0x00,0xa0,0x2f,0xf9,0x70,0x11,0,0,3,0,1,0,0,3,0,0x3c,0x8f,0x16,0x2e,0x48];
        assert_eq!(clip_resolution(&sps, Resolution::new(0, 0)).unwrap(), Resolution::new(640, 360));
        assert!(clip_resolution(&sps, Resolution::new(320, 240)).is_err());
        assert!(clip_resolution(&sps[..6], Resolution::new(0, 0)).is_err());
    }
}
