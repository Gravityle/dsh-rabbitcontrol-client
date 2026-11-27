//! 把控制端收到的**编码帧**交出去的解码器替身（tee）。
//!
//! 控制端管线（`rc_pipeline::ControllerPipeline`）固定按「重组 → 解码 →
//! 呈现」走，帧在 `Decoder::decode` 的入参里就已经是重组好的 Annex-B 码流，
//! 这是唯一能拿到编码帧的位置。本模块实现 [`Decoder`]，把码流写进宿主通道后
//! 返回 [`DecodeOutput::Nothing`]——不解码、不需要 GPU、不碰 Metal。
//!
//! 参考链语义按真实解码器来：首个 IDR 之前对 P 帧返回
//! [`DecodeOutput::NeedKeyframe`]，让会话层替我们向被控端请求关键帧。

use std::io::Write;

use rc_decode::{DecodeOutput, Decoder};
use rc_types::{FrameKind, Result, Timestamp};

/// 交给宿主的一帧编码数据。
pub struct EncodedFrame<'a> {
    /// Annex-B 码流。
    pub data: &'a [u8],
    /// 发送端声明的帧语义。
    pub kind: FrameKind,
    /// 自会话开始累计的第几帧（从 1 起）。
    pub index: u64,
}

/// 编码帧出口。实现者负责把它交到宿主进程（stdio、socket、队列……）。
pub trait FrameSink: Send {
    /// 交出一帧；返回 `Err` 表示通道已不可用。
    fn deliver(&mut self, frame: EncodedFrame<'_>) -> std::io::Result<()>;

    /// 会话结束时的收尾（冲刷缓冲）。
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 以 `[kind u8][len u32 BE][payload]` 成帧写出的出口。
///
/// `kind` 取值：1 = JSON 状态行，2 = Annex-B 视频帧（见 `main.rs` 的常量）。
pub struct FramedWriter<W: Write + Send> {
    out: W,
    frames: u64,
    bytes: u64,
}

impl<W: Write + Send> FramedWriter<W> {
    /// 包一个写入端。
    pub fn new(out: W) -> Self {
        Self { out, frames: 0, bytes: 0 }
    }

    /// 已交付的帧数与字节数。
    pub fn stats(&self) -> (u64, u64) {
        (self.frames, self.bytes)
    }

    /// 写一条状态行（`kind = 1`）。
    pub fn status(&mut self, json: &str) -> std::io::Result<()> {
        self.write_frame(STATUS_KIND, json.as_bytes())
    }

    fn write_frame(&mut self, kind: u8, payload: &[u8]) -> std::io::Result<()> {
        let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        self.out.write_all(&[kind])?;
        self.out.write_all(&len.to_be_bytes())?;
        self.out.write_all(payload)?;
        self.out.flush()
    }
}

impl<W: Write + Send> FrameSink for FramedWriter<W> {
    fn deliver(&mut self, frame: EncodedFrame<'_>) -> std::io::Result<()> {
        self.write_frame(VIDEO_KIND, frame.data)?;
        self.frames += 1;
        self.bytes += frame.data.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// 状态行在成帧流里的类型号。
pub const STATUS_KIND: u8 = 1;
/// 视频帧在成帧流里的类型号。
pub const VIDEO_KIND: u8 = 2;

/// 只把编码帧交给 [`FrameSink`] 的解码器替身。
pub struct TeeDecoder<S: FrameSink> {
    sink: S,
    /// 首个 IDR 之前为 true：让会话层去请求关键帧。
    need_keyframe: bool,
    /// 已交付帧数（诊断用）。
    #[allow(dead_code)]
    delivered: u64,
}

impl<S: FrameSink> TeeDecoder<S> {
    /// 以出口构造；起始状态为「等待关键帧」。
    pub fn new(sink: S) -> Self {
        Self { sink, need_keyframe: true, delivered: 0 }
    }
}

/// Accept a whole Annex-B packet only when every NAL is an SPS or PPS.
/// Zero bytes preceding a start code or trailing the final NAL are padding.
fn parameter_sets_only(data: &[u8]) -> bool {
    let prefix = [0, 0, 1];
    let Some(start) = data.windows(3).position(|w| w == prefix) else { return false };
    if data[..start].iter().any(|&byte| byte != 0) { return false; }
    let mut rest = &data[start + 3..];
    loop {
        let next = rest.windows(3).position(|w| w == prefix);
        let nal = &rest[..next.unwrap_or(rest.len())];
        let Some((&header, body)) = nal.split_first() else { return false };
        // Parameter sets must be reference NALs and the forbidden bit must be clear.
        if header & 0x80 != 0 || header & 0x60 == 0 { return false; }
        let minimum = match header & 0x1f {
            7 => 4, // profile, constraints, level, then at least the SPS id
            8 => 1,
            _ => return false,
        };
        if !body.iter().skip(minimum - 1).any(|&byte| byte != 0) { return false; }
        match next {
            Some(offset) => rest = &rest[offset + 3..],
            None => return true,
        }
    }
}

impl<S: FrameSink> Decoder for TeeDecoder<S> {
    fn decode(&mut self, data: &[u8], kind: FrameKind, _deadline_ts: Timestamp) -> Result<DecodeOutput> {
        // 参考链没建立之前，P 帧对下游没有意义：如实报告缺关键帧，别把废帧推出去。
        let waiting = self.need_keyframe && kind != FrameKind::Idr;
        if waiting && !parameter_sets_only(data) {
            return Ok(DecodeOutput::NeedKeyframe);
        }
        if kind == FrameKind::Idr {
            self.need_keyframe = false;
        }
        self.delivered += 1;
        let frame = EncodedFrame { data, kind, index: self.delivered };
        self.sink
            .deliver(frame)
            .map_err(|e| rc_types::Error::InvalidConfig(format!("编码帧出口写入失败: {e}")))?;
        // tee 不产出解码帧：管线据此不进入呈现路径。
        Ok(if waiting { DecodeOutput::NeedKeyframe } else { DecodeOutput::Nothing })
    }

    fn needs_keyframe(&self) -> bool {
        self.need_keyframe
    }

    fn flush(&mut self) -> Result<()> {
        self.sink
            .finish()
            .map_err(|e| rc_types::Error::InvalidConfig(format!("编码帧出口冲刷失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = b"\x00\x00\x00\x01\x67\x42\xc0\x1e\xd9\x00\xa0\x2f\xf9\x70\x11\x00\x00\x03\x00\x01\x00\x00\x03\x00\x3c\x8f\x16\x2e\x48";
    const PPS: &[u8] = b"\x00\x00\x01\x68\xcb\x83\xcb\x20";
    const P: &[u8] = b"\x00\x00\x01\x41\x88";
    const IDR: &[u8] = b"\x00\x00\x01\x65\x88";

    #[test]
    fn parameter_sets_reach_sink_while_waiting_for_idr() {
        let mut tee = TeeDecoder::new(FramedWriter::new(Vec::new()));
        let combined = [SPS, PPS].concat();
        let mut wire = Vec::new();
        for data in [SPS, PPS, combined.as_slice()] {
            assert!(matches!(tee.decode(data, FrameKind::P, Timestamp(0)), Ok(DecodeOutput::NeedKeyframe)));
            assert!(tee.needs_keyframe());
            wire.push(VIDEO_KIND);
            wire.extend_from_slice(&(data.len() as u32).to_be_bytes());
            wire.extend_from_slice(data);
        }
        assert_eq!(tee.sink.out, wire);
        for packet in [P.to_vec(), [PPS, P].concat(), [P, SPS].concat()] {
            assert!(matches!(tee.decode(&packet, FrameKind::P, Timestamp(0)), Ok(DecodeOutput::NeedKeyframe)));
            assert_eq!(tee.sink.out, wire);
        }
        assert!(matches!(tee.decode(IDR, FrameKind::Idr, Timestamp(0)), Ok(DecodeOutput::Nothing)));
        assert!(!tee.needs_keyframe());
        assert!(matches!(tee.decode(P, FrameKind::P, Timestamp(0)), Ok(DecodeOutput::Nothing)));
        assert_eq!(tee.sink.stats().0, 5);
    }

    #[test]
    fn rejects_malformed_annex_b_parameter_packets() {
        for packet in [
            vec![], [&b"junk"[..], PPS].concat(), vec![0, 0, 1, 0xe8, 1],
            vec![0, 0, 1, 8, 1], vec![0, 0, 1, 0x67, 0x42],
            vec![0, 0, 1, 0x68, 0], [PPS, &[0, 0, 1]].concat(),
        ] {
            assert!(!parameter_sets_only(&packet));
        }
    }
}
