use crate::RgbaImage;

/// The first `levels` mip levels of `base`, largest first.
pub fn mip_chain(base: RgbaImage, levels: usize) -> Vec<RgbaImage> {
    if levels == 0 {
        return Vec::new();
    }
    let mut chain = Vec::with_capacity(levels);
    chain.push(base);
    while chain.len() < levels {
        let next = halve(chain.last().expect("the chain starts with its base"));
        chain.push(next);
    }
    chain
}

fn halve(source: &RgbaImage) -> RgbaImage {
    let width = (source.width / 2).max(1);
    let height = (source.height / 2).max(1);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let taps = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
                let sx = (2 * x + dx).min(source.width - 1);
                let sy = (2 * y + dy).min(source.height - 1);
                let index = (sy * source.width + sx) as usize * 4;
                &source.pixels[index..index + 4]
            });
            for channel in 0..4 {
                let sum: u32 = taps.iter().map(|tap| u32::from(tap[channel])).sum();
                pixels.push(((sum + 2) / 4) as u8);
            }
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// Mip levels in a full `.edds` chain, down to 1x1.
pub fn edds_level_count(width: u32, height: u32) -> usize {
    (width.max(height).ilog2() + 1) as usize
}

/// Mip levels in a `.paa` chain: both sides keep halving while both are above 4.
pub fn paa_level_count(width: u32, height: u32) -> usize {
    let (mut width, mut height, mut levels) = (width, height, 1);
    while width > 4 && height > 4 {
        width /= 2;
        height /= 2;
        levels += 1;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_counts_follow_the_edds_and_paa_rules() {
        let edds = [
            ((136, 112), 8),
            ((1024, 223), 11),
            ((1920, 1137), 11),
            ((1, 1), 1),
        ];
        for ((width, height), levels) in edds {
            assert_eq!(
                edds_level_count(width, height),
                levels,
                "edds {width}x{height}"
            );
        }

        let paa = [
            ((64, 64), 5),
            ((64, 32), 4),
            ((256, 32), 4),
            ((32, 256), 4),
            ((512, 512), 8),
            ((2, 2), 1),
        ];
        for ((width, height), levels) in paa {
            assert_eq!(
                paa_level_count(width, height),
                levels,
                "paa {width}x{height}"
            );
        }
    }

    #[test]
    fn the_chain_keeps_the_base_buffer_instead_of_copying_it() {
        let base = RgbaImage {
            width: 16,
            height: 16,
            pixels: vec![0; 16 * 16 * 4],
        };
        let at = base.pixels.as_ptr();
        let chain = mip_chain(base, 3);

        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].pixels.as_ptr(), at);

        let empty = RgbaImage {
            width: 16,
            height: 16,
            pixels: vec![0; 16 * 16 * 4],
        };
        assert!(mip_chain(empty, 0).is_empty());
    }

    #[test]
    fn chain_halves_each_side_down_to_one_pixel() {
        let base = RgbaImage {
            width: 136,
            height: 112,
            pixels: vec![0; 136 * 112 * 4],
        };

        let sizes: Vec<(u32, u32)> = mip_chain(base, 8)
            .iter()
            .map(|level| (level.width, level.height))
            .collect();

        assert_eq!(
            sizes,
            vec![
                (136, 112),
                (68, 56),
                (34, 28),
                (17, 14),
                (8, 7),
                (4, 3),
                (2, 1),
                (1, 1)
            ]
        );
    }

    #[test]
    fn mip_averages_round_half_up_per_channel() {
        let base = RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0, 0, 0, 255, 1, 0, 0, 255, 1, 0, 0, 255, 1, 0, 0, 255],
        };

        let chain = mip_chain(base, 2);

        assert_eq!(chain[1].pixels, vec![1, 0, 0, 255]);
    }

    #[test]
    fn single_row_averages_its_first_two_pixels() {
        let base = RgbaImage {
            width: 3,
            height: 1,
            pixels: vec![10, 0, 0, 255, 21, 0, 0, 255, 200, 0, 0, 255],
        };

        let chain = mip_chain(base, 2);

        assert_eq!((chain[1].width, chain[1].height), (1, 1));
        assert_eq!(chain[1].pixels, vec![16, 0, 0, 255]);
    }
}
