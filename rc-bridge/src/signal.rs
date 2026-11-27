//! 客户端侧信令接入：登记到 rc-server → 请求连接目标设备 → 拿到打洞/中继凭据。
//!
//! 这是"从设备列表里选一台连"的第二层：设备列表（REST）只告诉你有哪些设备，
//! 真正要连上还需要信令层——本端登记上线、向目标发 `connect_request`，由服务端
//! 仲裁后下发 `punch_coord`（对端公网/局域网地址 + 一次性中继令牌）。拿到凭据之后
//! 才是打洞（`RCP2`）或走中继（`RCR1`）。
//!
//! 本模块只做**登记 + 取凭据**这一段，并把拿到的凭据如实打印出来；媒体路径的选路
//! （LAN → P2P → 中继）是下一步。

use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_client_core::server_config::ServerConfig;
use rc_client_core::signal_client::{SignalClientHandle, SignalClientOptions};

/// 信令接入参数。
pub struct SignalOptions {
    /// rc-server 的 REST 地址，形如 `https://127.0.0.1:21118`（用于推出信令/中继端口）。
    pub server_url: String,
    /// 账号 access token（信令 `register` 要它）。
    pub token: String,
    /// 本端设备 key（稳定标识；不同进程要用不同的 key，否则会互相顶掉）。
    pub device_key: String,
    /// 目标 9 位设备码（允许带空格）。
    pub target: String,
    /// 校验服务端 TLS 用的 CA PEM 文件（自签部署需要）。
    pub ca_pem_path: Option<String>,
    /// 等登记完成的上限。
    pub wait: Duration,
    /// 显式指定信令端口（None = 由 REST 端口推导为 api-2）。
    pub signal_port: Option<u16>,
}

/// 从 REST 地址推出信令/中继端口（服务端固定 21116/21117，API 端口取 URL 上的）。
fn parse_server_url(url: &str) -> Result<(String, u16, bool), String> {
    let trimmed = url.trim();
    let (https, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!("服务端地址要以 http:// 或 https:// 开头：{url}"));
    };
    let rest = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| format!("端口无效：{port}"))?;
            (host.to_string(), port)
        }
        None => (rest.to_string(), if https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("服务端地址里没有主机名：{url}"));
    }
    Ok((host, port, https))
}

/// 登记并请求连接目标设备，打印拿到的凭据。
///
/// @param options - 服务端地址、token、设备 key、目标设备码与等待上限。
pub fn resolve_target(options: &SignalOptions) -> Result<(), String> {
    let (host, api_port, https) = parse_server_url(&options.server_url)?;
    let ca_cert_pem = match &options.ca_pem_path {
        Some(path) => Some(
            std::fs::read_to_string(path).map_err(|e| format!("无法读 CA {path}: {e}"))?,
        ),
        None => None,
    };
    let signal_port = options.signal_port.unwrap_or_else(|| api_port.saturating_sub(2).max(1));
    let relay_port = signal_port.saturating_add(1);
    let cfg = ServerConfig {
        enabled: true,
        server_host: host.clone(),
        signal_port,
        relay_port,
        api_port,
        use_https: https,
        ca_cert_pem,
        device_key: options.device_key.clone(),
        ..ServerConfig::default()
    };
    eprintln!(
        "rc-bridge 信令：登记到 {host}:21116（设备 key {}…，目标 {})",
        &options.device_key.chars().take(12).collect::<String>(),
        options.target
    );

    let token = options.token.clone();
    let handle = SignalClientHandle::spawn_with_options(
        cfg,
        machine_name(),
        Vec::new(),
        SignalClientOptions {
            allow_remote_control: Some(false),
            token_provider: Some(Arc::new(move || Ok(token.clone()))),
            ..SignalClientOptions::default()
        },
        |_snapshot| {},
    );

    // 等登记完成：拿到 9 位设备码就算成功。
    let deadline = Instant::now() + options.wait;
    let mut device_id: Option<String> = None;
    while Instant::now() < deadline {
        while let Some(event) = handle.try_recv_device_status() {
            eprintln!(
                "rc-bridge 信令：设备状态 {} online={} hostname={}",
                event.device_id, event.online, event.hostname
            );
        }
        while let Some(status) = handle.try_recv_status() {
            eprintln!(
                "rc-bridge 信令：状态 {:?} device_id={:?} signal_addr={:?} error={:?}",
                status.state, status.device_id, status.signal_addr, status.error_message
            );
        }
        if let Some(reason) = handle.try_recv_kicked_reason() {
            return Err(format!("被服务端断开：{reason}"));
        }
        let snapshot = handle.status();
        if snapshot.device_id.is_some() {
            device_id = snapshot.device_id.clone();
            eprintln!(
                "rc-bridge 信令：已登记 device_id={}（{}）",
                snapshot.device_id_formatted.clone().unwrap_or_default(),
                snapshot.device_id.clone().unwrap_or_default()
            );
            break;
        }
        if let Some(error) = snapshot.error_message.clone() {
            eprintln!("rc-bridge 信令：等待登记中，最近错误：{error}");
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    if device_id.is_none() {
        return Err(format!(
            "在 {:?} 内没有登记成功（检查 token 是否有效、信令端口是否可达）",
            options.wait
        ));
    }

    // 请求连接目标：服务端仲裁后返回中继/打洞凭据。
    let grant = match handle.request_connect(&options.target, Duration::from_secs(15)) {
        Ok(grant) => grant,
        Err(error) => {
            return Err(format!("请求连接 {} 失败：{error}", options.target));
        }
    };
    eprintln!("rc-bridge 信令：拿到连接凭据");
    let payload = serde_json::json!({
        "event": "connect_grant",
        "selfDeviceId": device_id,
        "sessionId": grant.session_id,
        "targetId": grant.target_id,
        "relayAddr": grant.relay_addr,
        "relayTokenPrefix": grant.relay_token.chars().take(8).collect::<String>(),
        "peerPublicAddr": grant.peer_public_addr,
        "peerLanAddrs": grant.peer_lan_addrs,
        "localUdpPort": grant.local_udp_port,
    });
    println!("{payload}");
    Ok(())
}

/// 取宿主机名作为信令显示名：别再写死 "rc-bridge"，否则所有被控端在设备列表里同名。
fn machine_name() -> String {
    // macOS 上 `hostname` 给的是 `xxx.local`，而「电脑名称」更像用户在系统里看到的那个。
    if cfg!(target_os = "macos") {
        if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "rc-bridge".to_string()
}

/// 构造服务器配置（从 REST 地址推出信令/中继/API 端口）。
///
/// @param server_url - 形如 `https://127.0.0.1:21118`。
/// @param device_key - 本端设备 key。
/// @param ca_pem_path - 校验服务端 TLS 的 CA PEM 文件。
pub fn server_config(
    server_url: &str,
    device_key: &str,
    ca_pem_path: Option<&str>,
    signal_port_override: Option<u16>,
) -> Result<ServerConfig, String> {
    let (host, api_port, https) = parse_server_url(server_url)?;
    // 端口约定：REST = api_port，信令 = api_port-2，中继 = api_port-1（官方部署即 21118/21116/21117）。
    // 有些机器会把默认端口让给别的服务（例如同一台 VPS 上还跑着 RustDesk 占用 21115-21119），
    // 这时服务端整体错开两位，所以按 REST 端口推导；需要时用 `--signal-port` 覆盖。
    let signal_port = signal_port_override.unwrap_or_else(|| api_port.saturating_sub(2).max(1));
    let relay_port = signal_port.saturating_add(1);
    let ca_cert_pem = match ca_pem_path {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|e| format!("无法读 CA {path}: {e}"))?),
        None => None,
    };
    Ok(ServerConfig {
        enabled: true,
        server_host: host,
        signal_port,
        relay_port,
        api_port,
        use_https: https,
        ca_cert_pem,
        device_key: device_key.to_string(),
        ..ServerConfig::default()
    })
}

/// 起一个信令客户端并等它登记完成。
///
/// @param cfg - 服务器配置。
/// @param token - 账号 access token。
/// @param device_pubkey - 本机设备 Ed25519 公钥（可空）。
/// @param lan_addrs - 上报给服务端的局域网候选地址。
/// @param wait - 等登记完成的上限。
/// @returns 句柄与 9 位设备码。
pub fn register_and_wait(
    cfg: &ServerConfig,
    token: &str,
    device_pubkey: Option<String>,
    lan_addrs: Vec<String>,
    wait: Duration,
    allow_remote_control: bool,
) -> Result<(SignalClientHandle, String), String> {
    let token = token.to_string();
    let handle = SignalClientHandle::spawn_with_options(
        cfg.clone(),
        machine_name(),
        lan_addrs,
        SignalClientOptions {
            token_provider: Some(Arc::new(move || Ok(token.clone()))),
            allow_remote_control: Some(allow_remote_control),
            device_pubkey,
            ..SignalClientOptions::default()
        },
        |_snapshot| {},
    );
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        while let Some(event) = handle.try_recv_device_status() {
            eprintln!(
                "rc-bridge 信令：设备状态 {} online={} hostname={}",
                event.device_id, event.online, event.hostname
            );
        }
        while let Some(status) = handle.try_recv_status() {
            eprintln!(
                "rc-bridge 信令：状态 {:?} device_id={:?} relay_addr={:?} error={:?}",
                status.state, status.device_id, status.relay_addr, status.error_message
            );
        }
        if let Some(reason) = handle.try_recv_kicked_reason() {
            return Err(format!("被服务端断开：{reason}"));
        }
        let snapshot = handle.status();
        if let Some(device_id) = snapshot.device_id.clone() {
            eprintln!(
                "rc-bridge 信令：已登记 {}（{}）",
                snapshot.device_id_formatted.clone().unwrap_or_default(),
                device_id
            );
            return Ok((handle, device_id));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Err(format!("在 {wait:?} 内没有登记成功（检查 token、信令端口、CA）"))
}

/// 信令控制端（中继路径）：登记 → 签发设备证书 → 请求连接 → 走中继跑一次控制端会话。
///
/// @param options - 信令参数。
/// @param out - 帧流输出路径（`-` 表示 stdout）。
/// @param duration - 会话时长。
/// @param cert_dir - 设备密钥/证书目录。
#[allow(clippy::too_many_arguments)]
pub fn run_signaling_client(
    options: &SignalOptions,
    out: &str,
    duration: Duration,
    cert_dir: &std::path::Path,
    fps: u32,
    resolution: rc_types::Resolution,
    accept_input: bool,
    transport_choice: crate::TransportChoice,
) -> Result<(), String> {
    use rc_client_core::relay_transport::{RelayRole, SessionMtlsBundle};
    use rc_pipeline::{DiffMode, PipelineConfig};
    use rc_render::{FakeRenderer, FakeRendererConfig};
    use rc_session::{ClientAuthConfig, ClientSession, ClientSessionConfig};

    use crate::identity;
    use crate::relay;
    use crate::tee::TeeDecoder;

    let cfg = server_config(&options.server_url, &options.device_key, options.ca_pem_path.as_deref(), options.signal_port)?;
    let camera_dir = cert_dir.to_path_buf();
    let manager = rc_client_core::cert_manager::CertManager::load_or_init(camera_dir.clone())?;
    let pubkey = manager.device_pubkey_hex();
    let (handle, device_id) = register_and_wait(&cfg, &options.token, Some(pubkey), Vec::new(), options.wait, false)?;
    let identity = identity::provision(&cfg, &options.token, &device_id, cert_dir)?;

    let grant = handle
        .request_connect(&options.target, Duration::from_secs(15))
        .map_err(|e| format!("请求连接 {} 失败：{e}", options.target))?;
    eprintln!(
        "rc-bridge 信令：拿到凭据 relay={} peer_lan={:?} peer_public={:?}",
        grant.relay_addr, grant.peer_lan_addrs, grant.peer_public_addr
    );

    let mtls_for = |grant: &rc_client_core::signal_client::ConnectGrant| SessionMtlsBundle {
        ca_cert_pem: identity.ca_pem.clone(),
        cert_pem: identity.cert_pem.clone(),
        key_pem: identity.key_pem.clone(),
        local_user_id: identity.user_id.clone(),
        expected_peer_device_id: Some(grant.target_id.clone()),
        expected_peer_user_id: grant.is_owner_connection.then(|| identity.user_id.clone()),
    };
    use crate::p2p;
    use rc_client_core::p2p::P2pCandidates;

    let sink = crate::open_sink(out)?;
    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution,
        fps,
        bitrate_bps: 8_000_000,
        prefer_hardware: false,
        target_latency_us: 22_000,
        trace_capacity: 4096,
        diff: DiffMode::TrustPlatform,
        clock_origin_us: None,
        max_packet_payload: rc_pipeline::MAX_PAYLOAD,
        ..PipelineConfig::default()
    };

    // 在一条已经建好的传输上跑一次控制端会话，返回**本次**交付的帧数。
    //
    // 为什么要能跑第二次：家里的网络可能经代理/NAT 出海，打洞会"握手成功但媒体不通"。
    // 那时只看打洞是否报错是不够的，必须看有没有真的收到帧，再决定是否改走中继。
    let run_over = |transport: Box<dyn rc_net::Transport>, label: &str, with_input: bool, grant: &rc_client_core::signal_client::ConnectGrant| -> Result<u64, String> {
        let before = sink.lock().expect("帧出口锁").stats().0;
        let session_id = crate::derive_session_id();
        let session_cfg = ClientSessionConfig {
            peer_addr: "127.0.0.1:0".parse().expect("占位地址"),
            mtu: rc_session::DEFAULT_SESSION_MTU,
            requested: rc_session::proto::SessionParams {
                width: resolution.width,
                height: resolution.height,
                fps,
                bitrate_bps: 8_000_000,
            },
            session_id,
            pipeline: pipeline.clone(),
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(15),
            capture: if with_input {
                Box::new(crate::input::IpcCapture::spawn()) as Box<dyn rc_input::InputCapture>
            } else {
                Box::new(crate::IdleCapture) as Box<dyn rc_input::InputCapture>
            },
            input_window: rc_input::DEFAULT_WINDOW,
            auth: ClientAuthConfig {
                owner_signer: if grant.is_owner_connection {
                    let ticket = grant.owner_grant.clone().ok_or("同账号连接缺少服务端票据")?;
                    Some(Arc::new(manager.create_owner_ticket_signer(ticket)?))
                } else {
                    None
                },
                ..ClientAuthConfig::default()
            },
        };
        let t0 = Instant::now();
        let (clock, _anchor) = crate::reanchorable_clock(t0);
        let decoder = TeeDecoder::new(crate::SharedSink(Arc::clone(&sink)));
        let renderer = FakeRenderer::new(FakeRendererConfig {
            size: (resolution.width, resolution.height),
            ..FakeRendererConfig::default()
        })
        .map_err(|e| format!("假渲染器构造失败：{e}"))?;
        let mut client = ClientSession::with_parts(session_cfg, transport, Box::new(decoder), Box::new(renderer), clock)
            .map_err(|e| format!("控制端会话构造失败：{e}"))?;
        let negotiated = client.handshake().map_err(|e| format!("握手失败：{e}"))?;
        eprintln!(
            "rc-bridge 信令控制端（{label}）：已握手 {}x{} @ {}fps（session_id={session_id}）",
            negotiated.width, negotiated.height, negotiated.fps
        );

        let mut last_busy = true;
        while client.state() != rc_session::SessionState::Closed && t0.elapsed() < duration {
            match client.state() {
                rc_session::SessionState::Streaming => {
                    let budget = if last_busy {
                        crate::RUN_POLL_BUDGET
                    } else {
                        crate::RUN_IDLE_POLL_BUDGET
                    };
                    let report = client.tick(budget, Instant::now()).map_err(|e| format!("会话运行失败：{e}"))?;
                    if report.closed.is_some() {
                        break;
                    }
                    last_busy = report.busy;
                }
                rc_session::SessionState::Closed => break,
                _ => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        if client.state() != rc_session::SessionState::Closed {
            client.close(rc_session::proto::ByeReason::Normal).map_err(|e| format!("优雅收尾失败：{e}"))?;
        }
        let after = sink.lock().expect("帧出口锁").stats().0;
        let frames = after - before;
        eprintln!("rc-bridge 信令控制端（{label}）完成：本次 {frames} 帧 / {} 字节", sink.lock().expect("帧出口锁").stats().1);
        Ok(frames)
    };

    let candidates = || P2pCandidates {
        session_id: grant.session_id.clone(),
        peer_public_addr: grant.peer_public_addr.clone(),
        peer_lan_addrs: grant.peer_lan_addrs.clone(),
        local_udp_port: grant.local_udp_port,
    };
    let relay_of = |grant: &rc_client_core::signal_client::ConnectGrant, label: &str| -> Result<Box<dyn rc_net::Transport>, String> {
        eprintln!("rc-bridge 选路：{label} → 中继 {}", grant.relay_addr);
        relay::connect_relay(
            &grant.relay_addr,
            RelayRole::Client,
            &grant.relay_token,
            &mtls_for(grant),
            Duration::from_secs(10),
        )
    };

    // 第一次尝试：按偏好打洞或直接中继。
    let first: Box<dyn rc_net::Transport> = if transport_choice == crate::TransportChoice::Relay {
        relay_of(&grant, "按要求只用中继")?
    } else {
        match p2p::punch_client(&candidates(), &mtls_for(&grant), &p2p::PunchBudget::default()) {
            Ok(transport) => transport,
            Err(error) if transport_choice == crate::TransportChoice::P2p => {
                return Err(format!("按要求只用打洞，但打洞失败：{error}"));
            }
            Err(error) => relay_of(&grant, &format!("打洞失败（{error}）"))?,
        }
    };
    let label = if transport_choice == crate::TransportChoice::Relay { "中继" } else { "打洞" };
    let first_frames = run_over(first, label, accept_input, &grant)?;
    if first_frames > 0 {
        return Ok(());
    }
    if transport_choice == crate::TransportChoice::P2p {
        return Err("按要求只用打洞：会话没有交付任何编码帧".into());
    }
    if transport_choice == crate::TransportChoice::Relay {
        return Err("中继会话没有交付任何编码帧".into());
    }

    // 打洞握手成功却没有媒体：家里经代理出海时就是这样（对端地址是代理出口）。
    // 一次性令牌已被用掉，所以要重新 request_connect 拿新凭据，再走中继重试一次。
    eprintln!("rc-bridge 选路：打洞已连通但 0 帧（疑似代理/NAT 让媒体单向不通），重新取凭据改走中继");
    let retry = handle
        .request_connect(&options.target, Duration::from_secs(15))
        .map_err(|e| format!("中继重试时请求连接失败：{e}"))?;
    let transport = relay_of(&retry, "打洞无媒体，改走中继")?;
    let frames = run_over(transport, "中继重试", accept_input, &retry)?;
    if frames == 0 {
        return Err("打洞与中继都没有交付任何编码帧".into());
    }
    Ok(())
}
