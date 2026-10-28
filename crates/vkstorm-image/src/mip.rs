//! `LLImageBase::generateMip`: the 2x2 box filter the DXT encoder uses to
//! build its mip chain.

/// Average 2x2 blocks of `indata` into a `width` x `height` mip of
/// `nchannels` samples per pixel. The input is read with a row stride of
/// `2 * width` pixels, exactly as the C++ does (it never looks at the source
/// mip's own width). `None` when `nchannels` is not 1 to 4 or `indata` is too
/// short.
pub fn generate_mip(indata: &[u8], width: usize, height: usize, nchannels: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; width * height * nchannels];
    generate_mip_into(indata, &mut out, width, height, nchannels).then_some(out)
}

/// [`generate_mip`] into a caller-provided buffer of `width * height *
/// nchannels` bytes. `false` when the inputs do not fit.
pub fn generate_mip_into(indata: &[u8], out: &mut [u8], width: usize, height: usize, nchannels: usize) -> bool {
    if !(1..=4).contains(&nchannels) || out.len() < width * height * nchannels {
        return false;
    }
    let in_width = width * 2;
    let in_stride = in_width * nchannels;
    if height > 0 && width > 0 && indata.len() < 2 * height * in_stride {
        return false;
    }
    let out_stride = width * nchannels;
    for (h, dst_row) in out[..height * out_stride].chunks_exact_mut(out_stride).enumerate() {
        let row0 = &indata[2 * h * in_stride..2 * h * in_stride + in_stride];
        let row1 = &indata[(2 * h + 1) * in_stride..(2 * h + 2) * in_stride];
        for ((dst, a), c) in dst_row.chunks_exact_mut(nchannels).zip(row0.chunks_exact(2 * nchannels)).zip(row1.chunks_exact(2 * nchannels)) {
            let (b, d) = (&a[nchannels..], &c[nchannels..]);
            for k in 0..nchannels {
                dst[k] = ((a[k] as u32 + b[k] as u32 + c[k] as u32 + d[k] as u32) >> 2) as u8;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::generate_mip;

    #[test]
    fn averages_blocks() {
        // 4x2 single-channel source -> 2x1 mip.
        let src = [0u8, 4, 8, 12, 4, 8, 12, 16];
        assert_eq!(generate_mip(&src, 2, 1, 1).unwrap(), vec![4, 12]);
        // Truncating division, as in the C++.
        let src = [1u8, 2, 3, 5];
        assert_eq!(generate_mip(&src, 1, 1, 1).unwrap(), vec![2]);
        assert!(generate_mip(&src, 1, 1, 5).is_none());
        assert!(generate_mip(&src[..3], 1, 1, 1).is_none());
    }
}
