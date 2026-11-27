//! `rc-bridge`：RabbitControl 的**控制端**边车。
//!
//! 它做什么：以控制端身份连上一台被控端，把收到的**编码帧（Annex-B H.264）**
//! 按 `[kind u8][len u32 BE][payload]` 成帧写到 `--out`（默认 stdout），
//! 状态行（`kind = 1`）与视频帧（`kind = 2`）共用这条流；日志走 stderr。
//!
//! 它不做什么：不解码、不上屏、不注入输入——这些交给宿主进程（DSH 插件的
//! 浏览器侧用 WebCodecs 解码呈现）。因此它不需要 GPU、不需要 Metal、
//! 不需要任何 macOS 专属 crate。
//!
//! 里程碑 1 的验证目标：`rc-cli host --backend fake` 当被控端，本进程能连上
//! 并吐出真实帧；用 `--out frames.rcb` 落盘后可离线解析 NAL 类型。

mod clip;
mod host;
mod identity;
mod input;
mod p2p;
mod relay;
mod signal;
mod sps;
mod tee;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use rc_pipeline::{ControllerPipeline, DiffMode, PipelineConfig};
use rc_render::{FakeRenderer, FakeRendererConfig};
use rc_session::proto::{ByeReason, SessionParams};
use rc_session::{ClientAuthConfig, ClientSession, ClientSessionConfig, SessionClock, SessionState};
use rc_types::{FrameKind, Resolution};

use clip::{split_access_units, unit_kind};

use tee::{FrameSink, FramedWriter, TeeDecoder};

/// 常规 poll 预算（照 `rc-cli` 的分档）。
const RUN_POLL_BUDGET: Duration = Duration::from_millis(1);
/// 空闲拍放宽的 poll 预算。
const RUN_IDLE_POLL_BUDGET: Duration = Duration::from_millis(10);
/// 状态行的发送间隔。
const STATUS_INTERVAL: Duration = Duration::from_millis(1000);
/// 默认请求码率（bps）。
const DEFAULT_BITRATE_BPS: u32 = 8_000_000;

#[derive(Parser, Debug)]
#[command(name = "rc-bridge", about = "RabbitControl 控制端边车：把编码帧交给宿主进程")]
struct Args {
    /// 被控端地址，如 127.0.0.1:9901。与 --replay 二选一。
    #[arg(long)]
    connect: Option<SocketAddr>,
    /// 离线回放：直接读一个 Annex-B H.264 文件并按时序吐出（不发网络）。
    /// 用于在无被控端、无屏幕录制权限时验证下游的解码与渲染通路。
    #[arg(long)]
    replay: Option<PathBuf>,
    /// 当被控端：把这份 Annex-B 文件当作"屏幕"经 QUIC 发给控制端（循环播放）。
    /// 与 --replay / --connect 三选一。
    #[arg(long)]
    serve: Option<PathBuf>,
    /// `--serve` 的监听地址。
    #[arg(long, default_value = "127.0.0.1:9901")]
    listen: SocketAddr,
    /// 信令模式：要连接的目标 9 位设备码（配合 --signal-server 等使用）。
    #[arg(long)]
    signal_target: Option<String>,
    /// 信令模式：rc-server 的 REST 地址，形如 https://127.0.0.1:21118。
    #[arg(long)]
    signal_server: Option<String>,
    /// 信令模式：账号 access token。
    #[arg(long)]
    signal_token: Option<String>,
    /// 信令模式：本端设备 key（要与常驻设备区分，否则会互相顶掉）。
    #[arg(long, default_value = "rc-bridge-client")]
    signal_device_key: String,
    /// 信令模式：校验服务端 TLS 的 CA PEM 文件。
    #[arg(long)]
    signal_ca: Option<String>,
    /// 信令端口（默认 0 = 由 REST 端口推导为 api-2）。
    #[arg(long, default_value_t = 0)]
    signal_port: u16,
    /// 信令模式：等登记完成的上限。
    #[arg(long, default_value = "8s")]
    signal_wait: String,
    /// 信令模式：拿到凭据后接着走中继跑控制端会话（把帧写到 --out）。
    #[arg(long, default_value_t = false)]
    signal_connect: bool,
    /// 信令被控端：上报给服务端的局域网候选地址。
    #[arg(long, default_value = "127.0.0.1:9901")]
    signal_lan: String,
    /// 信令模式：设备密钥/证书目录（默认系统临时目录，Windows 上没有 /tmp）。
    #[arg(long)]
    signal_cert_dir: Option<PathBuf>,
        /// 信令被控端的内容来源：clip（回放片段，跨平台）| real（真实屏幕，仅 macOS 且带 real-capture 特性）。
    #[arg(long, default_value = "clip")]
    source: String,
    /// 传输偏好：auto（先打洞、失败走中继）| p2p（只用打洞）| relay（只用中继）。
    #[arg(long, default_value = "auto")]
    transport: String,
    /// 信令被控端：总运行时长（等连接 + 服务会话）。
    #[arg(long, default_value = "180s")]
    signal_overall: String,
    /// 会话时长（`10s` / `5`）。
    #[arg(long, default_value = "10s")]
    duration: String,
    /// 请求分辨率 auto（被控端原生像素）或 WxH（宽高上限）。
    #[arg(long, default_value = "auto")]
    resolution: String,
    /// 请求帧率。
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// 帧流输出路径；`-` 表示 stdout。
    #[arg(long, default_value = "-")]
    out: String,
    /// 固定信任的服务端证书 PEM 文件；省略则用内嵌开发证书（与 rc-cli 默认一致）。
    #[arg(long)]
    server_cert: Option<PathBuf>,
    /// 证书 SAN 名，须与证书匹配。
    #[arg(long, default_value = "rabbitcontrol")]
    server_name: String,
    /// 输入来源：`-` = 从 stdin 读宿主事件，`none` = 不发输入。
    #[arg(long, default_value = "-")]
    input: String,
}

/// 不发任何输入的控制端（本里程碑只取帧）。
struct IdleCapture;

impl rc_input::InputCapture for IdleCapture {
    fn next_events(&mut self, _timeout: Duration) -> rc_types::Result<Vec<rc_input::InputEvent>> {
        Ok(Vec::new())
    }
}

/// 可重锚的会话钟：从 `origin` 起算的微秒读数。
fn reanchorable_clock(origin: Instant) -> (SessionClock, Arc<Mutex<Instant>>) {
    let anchor = Arc::new(Mutex::new(origin));
    let clock: SessionClock = {
        let anchor = Arc::clone(&anchor);
        Arc::new(move || {
            let a = *anchor.lock().expect("会话钟锚点锁");
            u32::try_from(a.elapsed().as_micros()).unwrap_or(u32::MAX)
        })
    };
    (clock, anchor)
}

/// 线协议使用 0×0 请求被控端的当前像素尺寸。
fn parse_resolution(text: &str) -> Result<Resolution, String> {
    if text.trim().eq_ignore_ascii_case("auto") {
        return Ok(Resolution::new(0, 0));
    }
    let (w, h) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("无效的分辨率 {text:?}：应为 auto 或 WxH"))?;
    let width = w.trim().parse::<u32>().map_err(|_| format!("无效宽度 {w:?}"))?;
    let height = h.trim().parse::<u32>().map_err(|_| format!("无效高度 {h:?}"))?;
    if !(2..=16_384).contains(&width) || !(2..=16_384).contains(&height) {
        return Err("宽与高必须在 2 至 16384 之间".into());
    }
    Ok(Resolution::new(width, height))
}

/// 解析时长：`10s` / `500ms` / 裸数字按秒。
fn parse_duration(text: &str) -> Result<Duration, String> {
    let trimmed = text.trim();
    if let Some(ms) = trimmed.strip_suffix("ms") {
        return ms
            .trim()
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| format!("无效时长 {text:?}"));
    }
    let seconds = trimmed.strip_suffix('s').unwrap_or(trimmed);
    seconds
        .trim()
        .parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|_| format!("无效时长 {text:?}"))
}

/// 把出口包成可在管线工厂与会话循环之间共享的句柄。
struct SharedSink(Arc<Mutex<FramedWriter<Box<dyn Write + Send>>>>);

impl FrameSink for SharedSink {
    fn deliver(&mut self, frame: tee::EncodedFrame<'_>) -> std::io::Result<()> {
        self.0.lock().expect("帧出口锁").deliver(frame)
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.0.lock().expect("帧出口锁").finish()
    }
}

/// 建一个状态行写入器（与视频帧共用同一个出口）。
fn status_line(sink: &Arc<Mutex<FramedWriter<Box<dyn Write + Send>>>>, json: &str) -> std::io::Result<()> {
    sink.lock().expect("帧出口锁").status(json)
}

/// 成帧出口的共享句柄。
type SharedWriter = Arc<Mutex<FramedWriter<Box<dyn Write + Send>>>>;

/// 打开出口：`-` 表示 stdout，否则当文件建。
fn open_sink(path: &str) -> Result<SharedWriter, String> {
    if path == "-" {
        Ok(Arc::new(Mutex::new(FramedWriter::new(
            Box::new(BufWriter::new(std::io::stdout())) as Box<dyn Write + Send>,
        ))))
    } else {
        let file = File::create(path).map_err(|e| format!("无法创建 {path}: {e}"))?;
        Ok(Arc::new(Mutex::new(FramedWriter::new(
            Box::new(BufWriter::new(file)) as Box<dyn Write + Send>,
        ))))
    }
}

/// 离线回放：读 Annex-B 文件，按 `--fps` 的时序吐帧，不发网络。
fn run_replay(args: &Args) -> Result<(), String> {
    let path = args.replay.as_ref().expect("调用方已判定 --replay 存在");
    let fps = args.fps.max(1);
    let bytes = std::fs::read(path).map_err(|e| format!("无法读回放文件 {}: {e}", path.display()))?;
    let units = split_access_units(&bytes);
    if units.is_empty() {
        return Err(format!("{} 里没有解析出任何访问单元", path.display()));
    }
    let keyframes = units.iter().filter(|u| unit_kind(u) == FrameKind::Idr).count();
    eprintln!(
        "rc-bridge 回放：{} 个访问单元（关键帧 {keyframes}），按 {fps}fps 吐出",
        units.len()
    );

    let sink = open_sink(&args.out)?;
    let interval = Duration::from_micros(1_000_000 / u64::from(fps));
    let t0 = Instant::now();
    let mut bytes_out = 0u64;
    for (index, unit) in units.iter().enumerate() {
        let target = interval * (index as u32 + 1);
        let elapsed = t0.elapsed();
        if target > elapsed {
            std::thread::sleep(target - elapsed);
        }
        bytes_out += unit.len() as u64;
        sink.lock()
            .expect("帧出口锁")
            .deliver(tee::EncodedFrame { data: unit, kind: unit_kind(unit), index: index as u64 + 1 })
            .map_err(|e| format!("帧写入失败: {e}"))?;
        if index % (fps as usize).max(1) == 0 {
            let status = format!(
                "{{\"event\":\"status\",\"frames\":{},\"bytes\":{bytes_out},\"elapsedMs\":{}}}",
                index + 1,
                t0.elapsed().as_millis()
            );
            status_line(&sink, &status).map_err(|e| format!("状态行写入失败: {e}"))?;
        }
    }
    let done = format!(
        "{{\"event\":\"done\",\"frames\":{},\"bytes\":{bytes_out},\"elapsedMs\":{}}}",
        units.len(),
        t0.elapsed().as_millis()
    );
    status_line(&sink, &done).map_err(|e| format!("收尾状态写入失败: {e}"))?;
    eprintln!("rc-bridge 回放结束：{} 帧 / {bytes_out} 字节", units.len());
    Ok(())
}

fn run_session_once(args: &Args, connect: SocketAddr) -> Result<u64, String> {
    let resolution = parse_resolution(&args.resolution)?;
    let duration = parse_duration(&args.duration)?;

    let sink = open_sink(&args.out)?;

    let server_cert = match &args.server_cert {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("无法读服务端证书 {}: {e}", path.display()))?,
        None => rc_session::dev_cert::dev_server_cert_pem().to_string(),
    };

    let pipeline = PipelineConfig {
        codec: rc_encode::Codec::H264,
        resolution: Resolution::new(2, 2), // 握手后由裁定值覆盖。
        fps: args.fps,
        bitrate_bps: DEFAULT_BITRATE_BPS,
        // 不解码、不呈现，任何硬件后端都不需要。
        prefer_hardware: false,
        target_latency_us: 22_000,
        trace_capacity: 4096,
        diff: DiffMode::TrustPlatform,
        clock_origin_us: None,
        max_packet_payload: rc_pipeline::MAX_PAYLOAD,
        ..PipelineConfig::default()
    };
    let session_id = derive_session_id();
    let cfg = ClientSessionConfig {
        peer_addr: connect,
        mtu: rc_session::DEFAULT_SESSION_MTU,
        requested: SessionParams {
            width: resolution.width,
            height: resolution.height,
            fps: args.fps,
            bitrate_bps: DEFAULT_BITRATE_BPS,
        },
        session_id,
        pipeline: pipeline.clone(),
        connect_timeout: Duration::from_secs(10),
        handshake_timeout: Duration::from_secs(10),
        capture: if args.input == "none" {
            Box::new(IdleCapture) as Box<dyn rc_input::InputCapture>
        } else {
            Box::new(input::IpcCapture::spawn()) as Box<dyn rc_input::InputCapture>
        },
        input_window: rc_input::DEFAULT_WINDOW,
        auth: ClientAuthConfig::default(),
    };

    let t0 = Instant::now();
    let (clock, _anchor) = reanchorable_clock(t0);
    let clock_reader = Arc::clone(&clock);

    // 管线工厂：tee 解码器（把 Annex-B 交出去）+ 假渲染器（永不呈现）。
    let factory = {
        let sink = Arc::clone(&sink);
        Box::new(move |pcfg: &PipelineConfig| -> rc_types::Result<ControllerPipeline> {
            let decoder = TeeDecoder::new(SharedSink(Arc::clone(&sink)));
            let renderer = FakeRenderer::new(FakeRendererConfig {
                size: (2, 2), // TeeDecoder 不产出像素。
                ..FakeRendererConfig::default()
            })?;
            ControllerPipeline::with_backends(pcfg.clone(), Box::new(decoder), Box::new(renderer))
        })
    };

    let mut client = ClientSession::new(
        cfg,
        rc_session::dev_cert::DEV_CERT_PEM,
        rc_session::dev_cert::DEV_KEY_PEM,
        &server_cert,
        &args.server_name,
        clock,
    )
    .map_err(|e| format!("控制端会话构造失败: {e}"))?;
    client.set_pipeline_factory_for(factory);

    eprintln!("rc-bridge 连接中：{connect}（请求 {}x{} @ {}fps）", resolution.width, resolution.height, args.fps);
    client.connect_and_handshake().map_err(|e| format!("connect/握手失败: {e}"))?;
    let negotiated = client.negotiated().ok_or("握手完成却没有裁定值")?;
    eprintln!(
        "rc-bridge 已连接：裁定 {}x{} @ {}fps，session_id={session_id}，输入={}",
        negotiated.width,
        negotiated.height,
        negotiated.fps,
        if args.input == "none" { "关闭" } else { "stdin" }
    );

    let mut last_busy = true;
    let mut last_status = Instant::now();
    let mut datagrams = 0u64;

    while client.state() != SessionState::Closed && t0.elapsed() < duration {
        match client.state() {
            SessionState::Streaming => {
                let now_us = u64::from(clock_reader());
                client.pump_probes(now_us).map_err(|e| format!("探测泵失败: {e}"))?;
                client.start_bw_probe(now_us).map_err(|e| format!("带宽探测发起失败: {e}"))?;
                let budget = if last_busy { RUN_POLL_BUDGET } else { RUN_IDLE_POLL_BUDGET };
                let report = client.tick(budget, Instant::now()).map_err(|e| format!("会话运行失败: {e}"))?;
                if report.closed.is_some() {
                    break;
                }
                last_busy = report.busy;
                datagrams += report.datagrams_pushed as u64;
            }
            SessionState::Closed => break,
            other => {
                eprintln!("rc-bridge 会话状态：{other:?}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        if last_status.elapsed() >= STATUS_INTERVAL {
            last_status = Instant::now();
            let (frames, bytes) = sink.lock().expect("帧出口锁").stats();
            let status = format!(
                "{{\"event\":\"status\",\"sessionId\":{session_id},\"frames\":{frames},\"bytes\":{bytes},\"elapsedMs\":{},\"datagrams\":{datagrams}}}",
                t0.elapsed().as_millis()
            );
            status_line(&sink, &status).map_err(|e| format!("状态行写入失败: {e}"))?;
        }
    }

    if client.state() != SessionState::Closed {
        client.close(ByeReason::Normal).map_err(|e| format!("优雅收尾失败: {e}"))?;
    }

    let (frames, bytes) = sink.lock().expect("帧出口锁").stats();
    let done = format!(
        "{{\"event\":\"done\",\"sessionId\":{session_id},\"frames\":{frames},\"bytes\":{bytes},\"elapsedMs\":{},\"datagrams\":{datagrams}}}",
        t0.elapsed().as_millis()
    );
    status_line(&sink, &done).map_err(|e| format!("收尾状态写入失败: {e}"))?;
    eprintln!("rc-bridge 完成：交付 {frames} 帧 / {bytes} 字节");
    Ok(frames)
}

/// 直连一次；失败或"0 帧"就**重试同一地址一次**。
///
/// 为什么要重试：被控端（如 `rc-cli host`）的设计是 **accept 一条连接、服务完就退出**
/// （accept 等待上限 300s），启动脚本重启它之间存在空档；用户"断开后立刻重连"很容易
/// 正好落进空档——表现为连上但一帧不出，或直接连接被拒。重试一次即可覆盖。
/// 重试仍失败就交回上层：面板还有"换下一个地址 → 改走中继"的降级。
fn run_session(args: &Args, connect: SocketAddr) -> Result<(), String> {
    let delay = Duration::from_secs(2);
    let attempt = run_session_once(args, connect);
    let frames = match attempt {
        Ok(frames) if frames > 0 => return Ok(()),
        Ok(_) => {
            eprintln!("rc-bridge：本次会话 0 帧（被控端可能刚重启），{}s 后重试同一地址…", delay.as_secs());
            0
        }
        Err(error) => {
            eprintln!("rc-bridge：{error}；{}s 后重试同一地址…", delay.as_secs());
            0
        }
    };
    let _ = frames;
    std::thread::sleep(delay);
    let again = run_session_once(args, connect)?;
    if again == 0 {
        return Err("两次会话都没有交付任何编码帧".into());
    }
    Ok(())
}

/// 证书目录：命令行优先，否则系统临时目录下的固定子目录。
fn cert_dir_of(args: &Args) -> PathBuf {
    args.signal_cert_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("rc-bridge-certs"))
}

/// 解析 `--source`。
pub fn parse_source(value: &str) -> Result<host::HostSource, String> {
    match value {
        "clip" => Ok(host::HostSource::Clip),
        "real" => Ok(host::HostSource::Real),
        other => Err(format!("未知内容来源 {other:?}：可用 clip / real")),
    }
}

/// 传输偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportChoice {
    /// 先打洞，失败退回中继。
    Auto,
    /// 只用打洞。
    P2p,
    /// 只用中继。
    Relay,
}

/// 解析 `--transport`。
pub fn parse_transport(value: &str) -> Result<TransportChoice, String> {
    match value {
        "auto" => Ok(TransportChoice::Auto),
        "p2p" => Ok(TransportChoice::P2p),
        "relay" => Ok(TransportChoice::Relay),
        other => Err(format!("未知传输偏好 {other:?}：可用 auto / p2p / relay")),
    }
}

/// 取信令 token：优先命令行，其次环境变量 `RC_SIGNAL_TOKEN`（避免令牌出现在 `ps` 输出里）。
fn signal_token_of(args: &Args) -> Option<String> {
    args.signal_token
        .clone()
        .or_else(|| std::env::var("RC_SIGNAL_TOKEN").ok().filter(|value| !value.trim().is_empty()))
}

/// 信令被控端：把片段当作"屏幕"，经信令接客、走中继发流。
fn run_signaling_host_mode(args: &Args) -> Result<ExitCode, String> {
    let clip = args.serve.clone().expect("调用方已判定 --serve 存在");
    let options = host::SignalHostOptions {
        clip,
        resolution: parse_resolution(&args.resolution)?,
        fps: args.fps,
        bitrate_bps: DEFAULT_BITRATE_BPS,
        duration: parse_duration(&args.duration)?,
        server_url: args.signal_server.clone().ok_or("需要 --signal-server")?,
        token: signal_token_of(args).ok_or("需要 --signal-token 或 RC_SIGNAL_TOKEN")?,
        device_key: args.signal_device_key.clone(),
        ca_pem_path: args.signal_ca.clone(),
        lan_addr: args.signal_lan.clone(),
        cert_dir: cert_dir_of(args),
        wait: parse_duration(&args.signal_wait)?,
        overall: parse_duration(&args.signal_overall)?,
        transport: parse_transport(&args.transport)?,
        source: parse_source(&args.source)?,
        signal_port: if args.signal_port == 0 { None } else { Some(args.signal_port) },
    };
    host::run_signaling_host(&options)?;
    Ok(ExitCode::SUCCESS)
}

/// 信令模式：登记 + 请求连接目标；`--signal-connect` 时接着走中继跑控制端会话。
fn run_signal_mode(args: &Args) -> Result<ExitCode, String> {
    let target = args.signal_target.clone().expect("调用方已判定 --signal-target 存在");
    let options = signal::SignalOptions {
        server_url: args.signal_server.clone().ok_or("信令模式需要 --signal-server")?,
        token: signal_token_of(args).ok_or("信令模式需要 --signal-token 或 RC_SIGNAL_TOKEN")?,
        device_key: args.signal_device_key.clone(),
        target,
        ca_pem_path: args.signal_ca.clone(),
        wait: parse_duration(&args.signal_wait)?,
        signal_port: if args.signal_port == 0 { None } else { Some(args.signal_port) },
    };
    if args.signal_connect {
        signal::run_signaling_client(
            &options,
            &args.out,
            parse_duration(&args.duration)?,
            &cert_dir_of(args),
            args.fps,
            parse_resolution(&args.resolution)?,
            args.input != "none",
            parse_transport(&args.transport)?,
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    signal::resolve_target(&options)?;
    Ok(ExitCode::SUCCESS)
}

/// 当被控端：把片段当作"屏幕"发出。
fn run_clip_host(args: &Args) -> Result<(), String> {
    let clip = args.serve.as_ref().expect("调用方已判定 --serve 存在");
    let resolution = parse_resolution(&args.resolution)?;
    let duration = parse_duration(&args.duration)?;
    let options = host::ClipHostOptions {
        listen: args.listen,
        clip: clip.clone(),
        resolution,
        fps: args.fps,
        bitrate_bps: DEFAULT_BITRATE_BPS,
        duration,
    };
    host::run(&options)
}

/// 会话号：pid 与单调计数合成（控制端生成）。
fn derive_session_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    (u64::from(std::process::id()) << 20) | (n & 0xF_FFFF) | 0x100_000
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.serve.is_some() && args.signal_server.is_some() {
        return match run_signaling_host_mode(&args) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("rc-bridge 信令被控端错误：{message}");
                ExitCode::FAILURE
            }
        };
    }
    if args.signal_target.is_some() {
        return match run_signal_mode(&args) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("rc-bridge 信令错误：{message}");
                ExitCode::FAILURE
            }
        };
    }
    let outcome = match (&args.serve, &args.replay, &args.connect) {
        (Some(_), None, None) => run_clip_host(&args),
        (None, Some(_), None) => run_replay(&args),
        (None, None, Some(connect)) => run_session(&args, *connect),
        (None, None, None) => Err(
            "必须给出 --connect <地址>、--replay <文件>、--serve <文件> 或 --signal-target <设备码>"
                .to_string(),
        ),
        _ => Err("--connect / --replay / --serve 三者互斥，只能给一个".to_string()),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rc-bridge 错误：{message}");
            // 尽力把错误也写进帧流，宿主不必只靠 stderr 判断。
            let _ = writeln!(std::io::stderr(), "rc-bridge: {message}");
            ExitCode::FAILURE
        }
    }
}
