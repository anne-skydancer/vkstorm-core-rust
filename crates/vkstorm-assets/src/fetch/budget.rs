//! The JPEG 2000 byte budget per discard level (`LLImageJ2C::calcDataSizeJ2C`
//! and `calcDiscardLevelBytes`): what the fetcher asks the server for when
//! it wants a texture at a given discard level, and the discard level a
//! number of bytes is good for.

/// `MAX_DISCARD_LEVEL`.
pub const MAX_DISCARD_LEVEL: i32 = 5;
/// `FIRST_PACKET_SIZE`, also `calcHeaderSizeJ2C()`.
pub const FIRST_PACKET_SIZE: i32 = 600;
/// `MAX_IMG_PACKET_SIZE`.
pub const MAX_IMG_PACKET_SIZE: i32 = 1000;
/// `MAX_BLOCK_SIZE`: 64x64 code blocks.
pub const MAX_BLOCK_SIZE: i32 = 64;
/// `MIN_LAYER_SIZE`.
pub const MIN_LAYER_SIZE: i32 = 2000;
/// `MAX_IMAGE_DATA_SIZE` (4096 * 4096 * 8).
pub const MAX_IMAGE_DATA_SIZE: i32 = 4096 * 4096 * 8;
/// The default compression rate `calcDataSizeJ2C` assumes (`rate = 0.125f`).
pub const DEFAULT_RATE: f32 = 0.125;

/// `LLImageJ2C::calcDataSizeJ2C(w, h, comp, discard_level, rate)`: the
/// estimated number of bytes from the start of the codestream covering
/// discard levels down to `discard_level`.
pub fn calc_data_size_j2c(w: i32, h: i32, _comp: i32, discard_level: i32, rate: f32) -> i32 {
    const PRECISION: i64 = 8;
    const MAX_COMPONENTS: i64 = 4;
    const HARD_CAP: i32 = 12;
    let base_layer_area: i64 = MAX_BLOCK_SIZE as i64 * MAX_BLOCK_SIZE as i64;
    let bits_per_tile: i64 = MAX_COMPONENTS * PRECISION;
    let discard_layers = (5 - discard_level).max(0);
    let rate64 = rate as f64;
    let header_bytes = FIRST_PACKET_SIZE as i64;

    let scaled_bits = |layer_area: i64| -> i64 {
        let layer_bits = layer_area * bits_per_tile;
        (layer_bits as f64 * rate64).round() as i64
    };

    let mut total_bits: i64;
    if w <= 0 || h <= 0 {
        total_bits = scaled_bits(base_layer_area);
    } else {
        let surface = w as i64 * h as i64;
        let mut layer_area = base_layer_area;
        let mut nb_layers = 1;
        total_bits = scaled_bits(layer_area);
        while surface > layer_area && nb_layers < HARD_CAP {
            if nb_layers <= discard_layers {
                total_bits += scaled_bits(layer_area);
            }
            nb_layers += 1;
            layer_area *= 4;
        }
        {
            let max_dimension = w.max(h);
            let ratio = if max_dimension > 0 {
                max_dimension as f32 / MAX_BLOCK_SIZE as f32
            } else {
                0.0
            };
            let dimension_layers = if ratio > 0.0 {
                ((ratio.log2().floor() as i32) + 1).max(1)
            } else {
                1
            };
            if dimension_layers > nb_layers {
                let mut extra = dimension_layers.min(HARD_CAP) - nb_layers;
                while extra > 0 {
                    extra -= 1;
                    if nb_layers <= discard_layers {
                        total_bits += scaled_bits(layer_area);
                    }
                    nb_layers += 1;
                    layer_area *= 4;
                }
            }
        }
    }
    let mut est = total_bits / 8;
    est += header_bytes;
    est = est.clamp(0, i32::MAX as i64);
    est as i32
}

/// `LLImageJ2C::calcDiscardLevelBytes(bytes)`: the smallest discard level
/// the first `bytes` of a `w`x`h`x`comp` image cover, with the viewer's
/// `TextureReverseByteRange` percentage.
pub fn calc_discard_level_bytes(
    w: i32,
    h: i32,
    comp: i32,
    bytes: i32,
    reverse_byte_range_percent: i32,
) -> i32 {
    if bytes == 0 {
        return MAX_DISCARD_LEVEL;
    }
    let mut discard_level = 0;
    loop {
        let bytes_needed = calc_data_size_j2c(w, h, comp, discard_level, DEFAULT_RATE);
        if bytes >= (bytes_needed * reverse_byte_range_percent / 100) {
            break;
        }
        discard_level += 1;
        if discard_level >= MAX_DISCARD_LEVEL {
            break;
        }
    }
    discard_level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_grows_with_detail() {
        let d5 = calc_data_size_j2c(1024, 1024, 3, 5, DEFAULT_RATE);
        let d2 = calc_data_size_j2c(1024, 1024, 3, 2, DEFAULT_RATE);
        let d0 = calc_data_size_j2c(1024, 1024, 3, 0, DEFAULT_RATE);
        assert!(d5 < d2 && d2 < d0);
        // Unknown dimensions: one 64x64 layer plus the header.
        assert_eq!(
            calc_data_size_j2c(0, 0, 3, 5, DEFAULT_RATE),
            64 * 64 * 32 / 8 / 8 + 600
        );
        assert_eq!(
            calc_data_size_j2c(64, 64, 3, 5, DEFAULT_RATE),
            64 * 64 * 32 / 8 / 8 + 600
        );
    }

    #[test]
    fn discard_from_bytes() {
        assert_eq!(
            calc_discard_level_bytes(512, 512, 3, 0, 80),
            MAX_DISCARD_LEVEL
        );
        let full = calc_data_size_j2c(512, 512, 3, 0, DEFAULT_RATE);
        assert_eq!(calc_discard_level_bytes(512, 512, 3, full, 80), 0);
        assert_eq!(
            calc_discard_level_bytes(512, 512, 3, 700, 80),
            MAX_DISCARD_LEVEL
        );
    }
}
