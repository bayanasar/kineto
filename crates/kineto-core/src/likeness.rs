//! How alike two frames are in composition.
//!
//! A clip that starts on one keyframe and is told to close on another
//! moves between them if it can and dissolves if it cannot. Whether the
//! plan calls two shots the same setup says what was asked for; whether
//! the two locked keyframes look alike says what the provider will be
//! given. Both are scaled to a small grey picture of the same size, so
//! framing, light and the place of the subject count and detail does not,
//! and compared by their structural similarity (SSIM, Wang et al. 2004, in
//! 7 by 7 windows).

use image::{GrayImage, imageops::FilterType};

/// The picture both frames are compared at, 16:9.
const WIDTH: u32 = 128;
const HEIGHT: u32 = 72;
const WINDOW: u32 = 7;

/// How alike frames must be for a clip to move from one to the other
/// rather than dissolve. Calibrated on the live runs: the keyframe pairs
/// Veo dissolved between score 0.13 and 0.32, and a cut 0.21; frames of
/// one static setup seconds apart, 0.62 to 0.97. Two candidates MiniMax
/// drew for one shot score 0.24 to 0.37: keyframes drawn apart rarely
/// share a setup.
pub const SAME_SETUP: f64 = 0.5;

/// How alike the images `one` and `other` are, from 1 for the same picture
/// down; `None` when either is not an image this build reads (JPEG, PNG
/// or WebP).
#[must_use]
pub fn likeness(one: &[u8], other: &[u8]) -> Option<f64> {
    Some(ssim(&small(one)?, &small(other)?))
}

fn small(bytes: &[u8]) -> Option<GrayImage> {
    let image = image::load_from_memory(bytes).ok()?;
    Some(image::imageops::resize(
        &image.to_luma8(),
        WIDTH,
        HEIGHT,
        FilterType::Triangle,
    ))
}

/// The mean structural similarity of two pictures of the same size.
fn ssim(one: &GrayImage, other: &GrayImage) -> f64 {
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let count = f64::from(WINDOW * WINDOW);
    let mut total = 0.0;
    let mut windows = 0u32;
    for top in 0..=HEIGHT - WINDOW {
        for left in 0..=WIDTH - WINDOW {
            let (mut sum_a, mut sum_b, mut sum_aa, mut sum_bb, mut sum_ab) =
                (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in top..top + WINDOW {
                for x in left..left + WINDOW {
                    let a = f64::from(one.get_pixel(x, y)[0]);
                    let b = f64::from(other.get_pixel(x, y)[0]);
                    sum_a += a;
                    sum_b += b;
                    sum_aa += a * a;
                    sum_bb += b * b;
                    sum_ab += a * b;
                }
            }
            let (mean_a, mean_b) = (sum_a / count, sum_b / count);
            // Sample variances and covariance, as the reference does.
            let unbiased = count / (count - 1.0);
            let var_a = (sum_aa / count - mean_a * mean_a) * unbiased;
            let var_b = (sum_bb / count - mean_b * mean_b) * unbiased;
            let cov = (sum_ab / count - mean_a * mean_b) * unbiased;
            total += ((2.0 * mean_a * mean_b + C1) * (2.0 * cov + C2))
                / ((mean_a * mean_a + mean_b * mean_b + C1) * (var_a + var_b + C2));
            windows += 1;
        }
    }
    total / f64::from(windows)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgb, RgbImage};

    use super::*;

    /// A 160x90 JPEG of a place (`place` seeds its texture) with a lit
    /// figure at `x`, as wide as `size`.
    fn frame(place: u32, x: u32, size: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(160, 90, |px, py| {
            if px.abs_diff(x) < size / 2 && py.abs_diff(45) < size * 3 / 4 {
                Rgb([200, 170, 150])
            } else {
                let grain = (px / 4).wrapping_mul(73_856_093)
                    ^ (py / 4).wrapping_mul(19_349_663)
                    ^ place.wrapping_mul(83_492_791);
                let level = u8::try_from(30 + grain % 90).unwrap();
                Rgb([level, level, level.saturating_add(10)])
            }
        });
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    #[test]
    fn a_frame_is_like_itself_and_unlike_another_composition() {
        let figure = frame(1, 80, 30);
        assert!((likeness(&figure, &figure).unwrap() - 1.0).abs() < 1e-9);
        // The figure shifts a little in the same place: the same setup.
        assert!(likeness(&figure, &frame(1, 84, 30)).unwrap() > SAME_SETUP);
        // Another place, the figure elsewhere: another setup.
        assert!(likeness(&figure, &frame(2, 130, 60)).unwrap() < SAME_SETUP);
    }

    #[test]
    fn what_is_not_an_image_cannot_be_compared() {
        assert_eq!(
            likeness(b"\xff\xd8\xff\xe0JPEG-shaped", &frame(1, 80, 30)),
            None
        );
        assert_eq!(likeness(&frame(1, 80, 30), b""), None);
    }
}
