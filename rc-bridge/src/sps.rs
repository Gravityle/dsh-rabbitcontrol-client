//! SPS dimensions adapted from RabbitControl rc-decode-vt (AGPL-3.0-only).
/// 去除 SPS 载荷里的 emulation prevention 字节（`00 00 03` → `00 00`，
/// ITU-T H.264 §7.4.1）。
///
/// 载荷里的 `00 00 03` 是编码器为防止起始码误匹配插入的转义，解析语法
/// 元素前必须去掉。返回新分配的 Vec（输入通常不含转义字节，此时原样拷贝）。
fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut i = 0;
    while i < payload.len() {
        if i + 2 < payload.len() && payload[i] == 0 && payload[i + 1] == 0 && payload[i + 2] == 3 {
            out.extend_from_slice(&[0, 0]);
            i += 3;
        } else {
            out.push(payload[i]);
            i += 1;
        }
    }
    out
}

/// 位读取器（MSB 优先）。任何越界读取返回 `None`。
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // 位偏移
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 读 `n` 位（n ≤ 32）为 u32。
    fn bits(&mut self, n: u32) -> Option<u32> {
        if n > 32 || (self.pos + n as usize) > self.data.len() * 8 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.pos += 1;
        }
        Some(value)
    }

    /// 读 1 位为 bool。
    fn flag(&mut self) -> Option<bool> {
        self.bits(1).map(|v| v != 0)
    }

    /// 无符号 Exp-Golomb 码 ue(v)（H.264 §9.1）。前导零超过 31 位按畸形拒绝。
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let suffix = if zeros == 0 { 0 } else { self.bits(zeros)? };
        Some((1u64 << zeros) as u32 - 1 + suffix)
    }

    /// 有符号 Exp-Golomb 码 se(v)：ue(v) 的低 32 位映射（值域足够，溢出按 None 拒绝）。
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        if k == u32::MAX {
            return None; // 2^31-1+2^31 溢出 i32：畸形。
        }
        let k = k as i64;
        Some(if k % 2 == 1 {
            ((k + 1) / 2) as i32
        } else {
            -((k / 2) as i32)
        })
    }
}

/// 需要 chroma_format_idc / scaling matrix 的 profile（H.264 表 A-1 高档族）。
const HIGH_PROFILES: [u8; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// H.264 SPS display size after frame cropping.
pub(crate) fn sps_dimensions(sps_nal: &[u8]) -> Option<(u32, u32)> {
    // 跳过 NAL 头字节；去掉 emulation prevention 后按位解析。
    let body = strip_emulation_prevention(sps_nal.get(1..)?);
    let mut r = BitReader::new(&body);

    let profile_idc = r.bits(8)? as u8;
    let _constraint_flags = r.bits(8)?;
    let _level_idc = r.bits(8)?;
    let _sps_id = r.ue()?;

    // 高档族：额外的 chroma_format / bit_depth / scaling matrix 语法元素。
    let mut chroma_format_idc = 1u32; // 未出现时按 4:2:0（H.264 §7.4.2.1.1）
    if HIGH_PROFILES.contains(&profile_idc) {
        chroma_format_idc = r.ue()?;
        let mut separate_colour_planes = false;
        if chroma_format_idc == 3 {
            separate_colour_planes = r.flag()?;
        }
        if separate_colour_planes {
            chroma_format_idc = 0; // 单色平面按 monochrome 处理
        }
        let _bit_depth_luma = r.ue()?;
        let _bit_depth_chroma = r.ue()?;
        let _qp_prime_bypass = r.flag()?;
        if r.flag()? {
            let lists = if chroma_format_idc == 3 || separate_colour_planes { 12 } else { 8 };
            for index in 0..lists {
                if r.flag()? {
                    let mut last = 8i64;
                    let mut next = last;
                    for _ in 0..if index < 6 { 16 } else { 64 } {
                        if next != 0 { next = (last + i64::from(r.se()?)).rem_euclid(256); }
                        if next != 0 { last = next; }
                    }
                }
            }
        }
    }

    let _log2_max_frame_num = r.ue()?;
    let pic_order_cnt_type = r.ue()?;
    if pic_order_cnt_type == 0 {
        let _log2_max_poc_lsb = r.ue()?;
    } else if pic_order_cnt_type == 1 {
        let _delta_pic_order_always_zero = r.flag()?;
        let _offset_to_non_ref_pic = r.se()?;
        let _offset_to_top_bottom = r.se()?;
        let cycle = r.ue()?;
        for _ in 0..cycle {
            let _offset_for_ref: i32 = r.se()?;
        }
    } // poc_type == 2：无额外语法元素。

    let _max_num_ref_frames = r.ue()?;
    let _gaps_in_frame_num = r.flag()?;
    let pic_width_in_mbs_minus1 = r.ue()?;
    let pic_height_in_map_units_minus1 = r.ue()?;
    let frame_mbs_only_flag = r.flag()?;
    if !frame_mbs_only_flag {
        let _mb_adaptive = r.flag()?;
    }
    let _direct_8x8 = r.flag()?;
    let frame_cropping_flag = r.flag()?;
    let (crop_left, crop_right, crop_top, crop_bottom) = if frame_cropping_flag {
        (r.ue()?, r.ue()?, r.ue()?, r.ue()?)
    } else {
        (0, 0, 0, 0)
    };

    // 编码尺寸（宏块对齐）；frame_mbs_only=0（场宏块）时垂直方向 ×2。
    let frame_mbs_only = u32::from(frame_mbs_only_flag);
    let width_mbs = u64::from(pic_width_in_mbs_minus1) + 1;
    let height_map = u64::from(pic_height_in_map_units_minus1) + 1;
    let crop_unit_y_factor = 2 - frame_mbs_only;

    // 裁剪单位（H.264 §7.4.2.1.1，含 4:4:4 / monochrome 的 SubWidthC=1）。
    let (crop_unit_x, crop_unit_y) = match chroma_format_idc {
        1 => (2u64, 2 * u64::from(crop_unit_y_factor)), // 4:2:0
        2 => (2, u64::from(crop_unit_y_factor)),        // 4:2:2
        _ => (1, u64::from(crop_unit_y_factor)),        // 4:4:4 / monochrome
    };

    let width = width_mbs
        .checked_mul(16)?
        .checked_sub(u64::from(crop_left) * crop_unit_x)?
        .checked_sub(u64::from(crop_right) * crop_unit_x)?;
    let height = height_map
        .checked_mul(16 * u64::from(crop_unit_y_factor))?
        .checked_sub(u64::from(crop_top) * crop_unit_y)?
        .checked_sub(u64::from(crop_bottom) * crop_unit_y)?;

    // 溢出防护 + 非零校验（裁剪把整帧裁没 = 畸形 SPS）。
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}
