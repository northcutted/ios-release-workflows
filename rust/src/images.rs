use anyhow::{Context, Result, ensure};
use std::io::Cursor;

/// Encode screenshots as RGB PNGs accepted by App Store Connect.
pub fn store_png(input: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = png::Decoder::new(Cursor::new(input));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_limits(png::Limits {
        bytes: 128 * 1024 * 1024,
    });
    let mut reader = decoder.read_info()?;
    let size = reader.output_buffer_size().context("PNG is too large")?;
    ensure!(size <= 128 * 1024 * 1024, "PNG is too large");
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer)?;
    ensure!(
        info.bit_depth == png::BitDepth::Eight,
        "Unsupported PNG depth"
    );
    let channels = info.color_type.samples();
    let data = &buffer[..info.buffer_size()];
    let mut rgb = Vec::with_capacity((info.width as usize) * (info.height as usize) * 3);
    for pixel in data.chunks_exact(channels) {
        let (color, alpha) = match info.color_type {
            png::ColorType::Rgb => ([pixel[0], pixel[1], pixel[2]], 255),
            png::ColorType::Rgba => ([pixel[0], pixel[1], pixel[2]], pixel[3]),
            png::ColorType::Grayscale => ([pixel[0]; 3], 255),
            png::ColorType::GrayscaleAlpha => ([pixel[0]; 3], pixel[1]),
            _ => anyhow::bail!("Unsupported screenshot color format"),
        };
        for value in color {
            rgb.push(
                ((value as u32 * alpha as u32 + 255 * (255 - alpha as u32) + 127) / 255) as u8,
            );
        }
    }
    let mut output = vec![];
    {
        let mut encoder = png::Encoder::new(&mut output, info.width, info.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&rgb)?;
    }
    Ok(output)
}
