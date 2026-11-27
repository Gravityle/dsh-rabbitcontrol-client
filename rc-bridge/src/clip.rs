//! 从 Annex-B 文件产出「编码帧」的直通编码器。
//!
//! 被控端管线的形状是「采集 → 编码 → 分片发送」。要让它在**没有屏幕录制权限**
//! 的情况下发出**真实可解码**的 H.264，做法是实现 [`Encoder`]：忽略采集来的像素，
//! 按顺序把文件里的访问单元交出去。采集用 `rc_capture::FakeBackend`（只负责按帧率
//! 触发），像素内容无人关心。
//!
//! 这样下游（WebCodecs / VideoToolbox）拿到的是真码流，而不是合成填充。

use std::time::Instant;

use rc_capture::{CaptureBackend, CapturedFrame, FakeBackend, FakeBackendConfig, FakeContent};
use rc_encode::{BitstreamFormat, Codec, EncodedFrame, Encoder, EncoderCaps, EncoderConfig};
use rc_types::{Error, FrameKind, Resolution, Result, Timestamp};

/// 找出每个 NAL 单元的（起始码起点，NAL 头字节下标）。
pub fn nal_units(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            out.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// 按访问单元（AU）切分 Annex-B 流。
///
/// 有 AUD（NAL 9）时以 AUD 为界——这是 AUD 的本义；没有 AUD 时退化为「一个 slice
/// NAL 一帧」，只适用于每帧单 slice 的流。
pub fn split_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    let nals = nal_units(data);
    if nals.is_empty() {
        return Vec::new();
    }
    let has_aud = nals
        .iter()
        .any(|(_, header)| data.get(*header).is_some_and(|b| b & 0x1f == 9));
    let mut units: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for (index, (start, header)) in nals.iter().enumerate() {
        let end = nals.get(index + 1).map_or(data.len(), |(next, _)| *next);
        let nal_type = data[*header] & 0x1f;
        let boundary = if has_aud { nal_type == 9 } else { nal_type == 1 || nal_type == 5 };
        if boundary && !current.is_empty() {
            units.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(&data[*start..end]);
    }
    if !current.is_empty() {
        units.push(current);
    }
    units
}

/// 一个访问单元是不是关键帧（含 IDR slice，NAL 5）。
pub fn unit_kind(unit: &[u8]) -> FrameKind {
    let key = nal_units(unit)
        .iter()
        .any(|(_, header)| unit.get(*header).is_some_and(|b| b & 0x1f == 5));
    if key {
        FrameKind::Idr
    } else {
        FrameKind::P
    }
}

/// 采集侧：假后端，只负责按帧率产出「有变化的帧」，像素内容直通编码器不读。
pub fn fake_capture(resolution: Resolution, fps: u32) -> Result<Box<dyn CaptureBackend>> {
    Ok(Box::new(FakeBackend::new(FakeBackendConfig {
        display: rc_capture::DisplayInfo {
            id: rc_types::DisplayId(0),
            resolution,
            scale: 1.0,
            hdr: false,
        },
        fps,
        content: FakeContent::Dynamic,
        dirty_rect: None,
    })?))
}

/// 直通编码器：把文件里的访问单元按顺序（循环）当作编码结果交出去。
pub struct ClipEncoder {
    units: Vec<Vec<u8>>,
    kinds: Vec<FrameKind>,
    next: usize,
    caps: EncoderCaps,
    fps: u32,
    encoded: u64,
    want_idr: bool,
    loops: u64,
}

impl ClipEncoder {
    /// 用一份 Annex-B 码流构造。
    ///
    /// @param bytes - 文件内容。
    /// @param cfg - 管线给出的编码配置（取分辨率/帧率用于能力描述与时间戳）。
    pub fn new(bytes: &[u8], cfg: &EncoderConfig) -> Result<Self> {
        let units = split_access_units(bytes);
        if units.is_empty() {
            return Err(Error::InvalidConfig("码流里没有解析出任何访问单元".into()));
        }
        let kinds: Vec<FrameKind> = units.iter().map(|unit| unit_kind(unit)).collect();
        if !kinds.contains(&FrameKind::Idr) {
            return Err(Error::InvalidConfig("码流里没有关键帧，控制端无法开始解码".into()));
        }
        Ok(Self {
            units,
            kinds,
            next: 0,
            caps: EncoderCaps {
                codec: Codec::H264,
                hardware: false,
                max_resolution: cfg.resolution,
                bitstream_format: BitstreamFormat::AnnexB,
                profiles: &[66],
            },
            fps: cfg.fps.max(1),
            encoded: 0,
            want_idr: false,
            loops: 0,
        })
    }

    /// 访问单元总数与关键帧数（启动日志用）。
    pub fn shape(&self) -> (usize, usize) {
        (self.units.len(), self.kinds.iter().filter(|kind| **kind == FrameKind::Idr).count())
    }

    /// 向前找到下一个关键帧下标（找不到就回到 0）。
    fn seek_idr(&mut self) -> Result<()> {
        let total = self.units.len();
        for offset in 0..total {
            let index = (self.next + offset) % total;
            if self.kinds[index] == FrameKind::Idr {
                self.next = index;
                return Ok(());
            }
        }
        Err(Error::InvalidConfig("码流里没有关键帧".into()))
    }
}

impl Encoder for ClipEncoder {
    fn encode(
        &mut self,
        _frame: &CapturedFrame,
        force_idr: bool,
        _deadline_ts: Timestamp,
    ) -> Result<EncodedFrame> {
        if force_idr || self.want_idr {
            self.seek_idr()?;
            self.want_idr = false;
        }
        if self.next >= self.units.len() {
            self.next = 0;
            self.loops += 1;
        }
        let index = self.next;
        self.next += 1;
        let kind = self.kinds[index];
        self.encoded += 1;
        Ok(EncodedFrame {
            data: self.units[index].clone(),
            kind,
            is_keyframe: kind == FrameKind::Idr,
            // 合成时间线：按帧率推进，保证单调，便于对端做粗略计时。
            encode_done_ts: Timestamp(
                u32::try_from(self.encoded.saturating_mul(1_000_000) / u64::from(self.fps))
                    .unwrap_or(u32::MAX),
            ),
        })
    }

    fn capabilities(&self) -> EncoderCaps {
        self.caps
    }

    fn set_bitrate(&mut self, bps: u32) -> Result<()> {
        if bps == 0 {
            return Err(Error::InvalidConfig("码率不能为 0".into()));
        }
        // 直通编码不改变码流，接受即可（对端据此协商，不影响我们发什么）。
        Ok(())
    }

    fn request_idr(&mut self) {
        self.want_idr = true;
    }

    fn flush(&mut self) -> Result<Vec<EncodedFrame>> {
        Ok(Vec::new())
    }
}

/// 采集返回的帧时间戳用会话钟兜底，避免依赖假后端内部时钟。
pub struct AnchoredCapture {
    inner: Box<dyn CaptureBackend>,
    origin: Instant,
}

impl AnchoredCapture {
    /// 包一层，把帧时间戳钉在 `origin` 起算的会话钟上。
    pub fn new(inner: Box<dyn CaptureBackend>, origin: Instant) -> Self {
        Self { inner, origin }
    }
}

impl CaptureBackend for AnchoredCapture {
    fn next_frame(&mut self, timeout: std::time::Duration) -> Result<Option<CapturedFrame>> {
        match self.inner.next_frame(timeout)? {
            Some(mut frame) => {
                frame.timestamp = Timestamp(
                    u32::try_from(self.origin.elapsed().as_micros()).unwrap_or(u32::MAX),
                );
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }

    fn poll_invalidation(&mut self) -> Option<rc_capture::InvalidationReason> {
        self.inner.poll_invalidation()
    }

    fn rebuild(&mut self) -> Result<()> {
        self.inner.rebuild()
    }

    fn display_info(&self) -> rc_capture::DisplayInfo {
        self.inner.display_info()
    }
}
