//! The window icon: a black key on a yellow tile, drawn here so that no image file or
//! resource compiler is needed. It matches the mark in the app's sidebar.

/// The icon as RGBA bytes, `size` × `size` pixels.
pub fn rgba(size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Average a grid of samples per pixel so the edges are smooth.
            let mut sum = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let [r, g, b, a] = sample(px, py);
                    // Colours are weighted by coverage so transparent samples do not darken edges.
                    sum[0] += r * a;
                    sum[1] += g * a;
                    sum[2] += b * a;
                    sum[3] += a;
                }
            }
            let count = (SAMPLES * SAMPLES) as f32;
            let alpha = sum[3] / count;
            for channel in &sum[..3] {
                let value = if sum[3] > 0.0 { channel / sum[3] } else { 0.0 };
                out.push((value * 255.0).round() as u8);
            }
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

/// The sizes in the icon file: what Windows asks for at 100% to 250% scaling, in the title
/// bar, the taskbar, the Start menu and Explorer's large views.
#[cfg(test)]
const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// The icon as an `.ico` file. The copy the program and the installer use is
/// `assets/vault-key.ico`; a test keeps that file equal to this.
#[cfg(test)]
fn ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = ICO_SIZES.iter().map(|&size| ico_image(size)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (&size, image) in ICO_SIZES.iter().zip(&images) {
        // A width or height of 256 is written as 0.
        out.extend_from_slice(&[size as u8, size as u8, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in &images {
        out.extend_from_slice(image);
    }
    out
}

/// One image of the icon file: a 32-bit bitmap, bottom row first, followed by its
/// one-bit transparency mask.
#[cfg(test)]
fn ico_image(size: u32) -> Vec<u8> {
    let pixels = rgba(size);
    let mask_row = size.div_ceil(32) as usize * 4;
    let mut out = Vec::new();
    for field in [40, size, size * 2] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    out.extend_from_slice(&[1, 0, 32, 0]);
    for field in [0, size * size * 4, 0, 0, 0, 0] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    let row = |y: u32| &pixels[(y * size * 4) as usize..((y + 1) * size * 4) as usize];
    for y in (0..size).rev() {
        for pixel in row(y).as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    for y in (0..size).rev() {
        let mut bits = vec![0u8; mask_row];
        for (x, pixel) in row(y).as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] == 0 {
                bits[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&bits);
    }
    out
}

const INK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.78, 0.17, 1.0]; // #FFC72C
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// True inside a square with rounded corners, centred in the unit square.
fn rounded_square(x: f32, y: f32, half: f32, radius: f32) -> bool {
    let dx = ((x - 0.5).abs() - (half - radius)).max(0.0);
    let dy = ((y - 0.5).abs() - (half - radius)).max(0.0);
    (x - 0.5).abs() <= half && (y - 0.5).abs() <= half && dx * dx + dy * dy <= radius * radius
}

/// The colour at a point of the unit square.
fn sample(x: f32, y: f32) -> [f32; 4] {
    if !rounded_square(x, y, 0.47, 0.12) {
        return CLEAR;
    }
    if !rounded_square(x, y, 0.39, 0.07) {
        return INK;
    }
    // The key lies along the diagonal, bow at the lower left; work in its own axes.
    let (dx, dy) = (x - 0.5, y - 0.5);
    let along = (dx - dy) * std::f32::consts::FRAC_1_SQRT_2; // towards the upper right
    let across = (dx + dy) * std::f32::consts::FRAC_1_SQRT_2;
    let bow = ((along + 0.17).powi(2) + across.powi(2)).sqrt();
    let in_bow = (0.065..=0.15).contains(&bow);
    let in_shaft = (-0.05..=0.30).contains(&along) && across.abs() <= 0.042;
    let in_teeth = (0.0..=0.115).contains(&across) && ((0.13..=0.185).contains(&along) || (0.235..=0.30).contains(&along));
    if in_bow || in_shaft || in_teeth {
        INK
    } else {
        YELLOW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(data: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }

    #[test]
    fn the_icon_is_a_dark_key_on_a_yellow_tile_with_clear_corners() {
        let size = 64;
        let data = rgba(size);
        assert_eq!(data.len(), (size * size * 4) as usize);
        assert_eq!(pixel(&data, size, 0, 0)[3], 0, "the corner is transparent");
        assert_eq!(pixel(&data, size, 32, 4), [0, 0, 0, 255], "the border is ink");
        assert_eq!(pixel(&data, size, 14, 14), [255, 199, 43, 255], "the tile is yellow");
        assert_eq!(pixel(&data, size, 34, 30), [0, 0, 0, 255], "the shaft crosses the middle");
        // Set GK_ICON_DUMP to a file path to write the raw pixels for a look.
        if let Some(path) = std::env::var_os("GK_ICON_DUMP") {
            std::fs::write(path, rgba(256)).unwrap();
        }
    }

    /// The icon file in the repo is the drawing above. Run this test with `GK_WRITE_ICON=1`
    /// to write the file again after changing the drawing.
    #[test]
    fn the_icon_file_matches_the_drawing() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("vault-key.ico");
        let drawn = ico();
        if std::env::var_os("GK_WRITE_ICON").is_some() {
            std::fs::write(&path, &drawn).unwrap();
        }
        assert_eq!(u16::from_le_bytes([drawn[4], drawn[5]]) as usize, ICO_SIZES.len());
        assert!(std::fs::read(&path).unwrap() == drawn, "{} is out of date; run this test with GK_WRITE_ICON=1", path.display());
    }
}
