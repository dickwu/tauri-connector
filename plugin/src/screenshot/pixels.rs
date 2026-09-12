use super::geometry::{mapping, rect_pixels};
use super::{
    Geometry, MAX_ENCODED_BYTES, MAX_RAW_BYTES, PixelOrder, Probe, RawFrame, RectCss,
    check_dimensions, elapsed_stage,
};
use image::{ImageEncoder, Rgba, RgbaImage};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::time::Instant;

pub(super) fn decode(frame: RawFrame) -> Result<RgbaImage, String> {
    match frame {
        RawFrame::Encoded(bytes) => {
            if bytes.len() > MAX_ENCODED_BYTES {
                return Err("capture_budget_exceeded: encoded bytes".into());
            }
            let reader = image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|_| "capture_decode_failed")?;
            let (width, height) = reader
                .into_dimensions()
                .map_err(|_| "capture_decode_failed: dimensions")?;
            check_dimensions(width, height)?;
            let mut reader = image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|_| "capture_decode_failed")?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(width);
            limits.max_image_height = Some(height);
            limits.max_alloc = Some(MAX_RAW_BYTES as u64);
            reader.limits(limits);
            reader
                .decode()
                .map(|image| image.to_rgba8())
                .map_err(|_| "capture_decode_failed: image or allocation limit".into())
        }
        RawFrame::Bitmap {
            bytes,
            width,
            height,
            stride,
            order,
            premultiplied,
            opaque,
        } => {
            check_dimensions(width, height)?;
            if bytes.len() > MAX_RAW_BYTES
                || stride < width as usize * 4
                || stride
                    .checked_mul(height as usize)
                    .is_none_or(|size| size > bytes.len())
            {
                return Err("capture_budget_exceeded: invalid bitmap stride/buffer".into());
            }
            let mut image = RgbaImage::new(width, height);
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let offset = y * stride + x * 4;
                    let pixel = &bytes[offset..offset + 4];
                    let mut rgba = match order {
                        PixelOrder::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                        PixelOrder::Bgra => [pixel[2], pixel[1], pixel[0], pixel[3]],
                        PixelOrder::Argb => [pixel[1], pixel[2], pixel[3], pixel[0]],
                        PixelOrder::Abgr => [pixel[3], pixel[2], pixel[1], pixel[0]],
                    };
                    if opaque {
                        rgba[3] = 255;
                    }
                    if premultiplied && rgba[3] != 255 {
                        let alpha = u32::from(rgba[3]);
                        for channel in &mut rgba[..3] {
                            *channel = (u32::from(*channel) * 255 + alpha / 2)
                                .checked_div(alpha)
                                .unwrap_or(0)
                                .min(255) as u8;
                        }
                    }
                    image.put_pixel(x as u32, y as u32, Rgba(rgba));
                }
            }
            Ok(image)
        }
    }
}

/// Mask before cropping, resampling, encoding, base64, or handing bytes out.
pub(super) fn protect(
    frame: RawFrame,
    probe: &Probe,
    target: Option<RectCss>,
    max_width: Option<u32>,
) -> Result<(Vec<u8>, Geometry, u32, u32, Value), String> {
    if !probe.ready || !probe.uncovered_reasons.is_empty() {
        return Err("redaction_unavailable".into());
    }
    let started = Instant::now();
    let mut image = decode(frame)?;
    let decode_timing = elapsed_stage(started);
    let geometry = mapping(
        &probe.viewport,
        image.width(),
        image.height(),
        target,
        max_width,
    )?;
    let sx = f64::from(image.width()) / probe.viewport.width_css;
    let sy = f64::from(image.height()) / probe.viewport.height_css;
    let started = Instant::now();
    for rect in &probe.sensitive_rects {
        let ([x, y, width, height], _) = rect_pixels(*rect, sx, sy, image.width(), image.height())?;
        for py in y..y + height {
            for px in x..x + width {
                image.put_pixel(px, py, Rgba([0, 0, 0, 255]));
            }
        }
    }
    let mask_timing = elapsed_stage(started);
    let started = Instant::now();
    let [x, y, width, height] = geometry.crop_px;
    let mut image = image::imageops::crop_imm(&image, x, y, width, height).to_image();
    if let Some(max) = max_width.filter(|max| *max < width) {
        let new_height = (f64::from(height) * f64::from(max) / f64::from(width))
            .round()
            .max(1.0) as u32;
        // Nearest filtering cannot bleed adjacent unmasked pixels into masks.
        image = image::imageops::resize(
            &image,
            max,
            new_height,
            image::imageops::FilterType::Nearest,
        );
    }
    let width = image.width();
    let height = image.height();
    let crop_resize_timing = elapsed_stage(started);
    let started = Instant::now();
    let mut sink = BoundedWriter(Vec::new());
    image::codecs::png::PngEncoder::new(&mut sink)
        .write_image(
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|_| "capture_encode_failed_or_budget_exceeded")?;
    let encode_timing = elapsed_stage(started);
    Ok((
        sink.0,
        geometry,
        width,
        height,
        json!({"clock":"monotonic", "decode":decode_timing,
        "mask":mask_timing,"cropResize":crop_resize_timing,"encode":encode_timing}),
    ))
}

struct BoundedWriter(Vec<u8>);

pub(super) fn transcode(
    png: Vec<u8>,
    format: &str,
    quality: u8,
) -> Result<(Vec<u8>, Value), String> {
    if format == "png" {
        return Ok((png, super::not_applicable_stage("png_output")));
    }
    let conversion_started = Instant::now();
    let started = Instant::now();
    let image = decode(RawFrame::Encoded(png))?;
    let decode_timing = elapsed_stage(started);
    let started = Instant::now();
    let mut sink = BoundedWriter(Vec::new());
    match format {
        "jpeg" | "jpg" => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut sink, quality)
            .encode_image(&image::DynamicImage::ImageRgba8(image).to_rgb8())
            .map_err(|_| "capture_encode_failed_or_budget_exceeded")?,
        "webp" => image::codecs::webp::WebPEncoder::new_lossless(&mut sink)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|_| "capture_encode_failed_or_budget_exceeded")?,
        _ => return Err("invalid_arguments: image format".into()),
    }
    let encode_timing = elapsed_stage(started);
    let mut timing = elapsed_stage(conversion_started);
    timing["decode"] = decode_timing;
    timing["encode"] = encode_timing;
    Ok((sink.0, timing))
}
impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(buf.len()) > MAX_ENCODED_BYTES {
            return Err(std::io::Error::other("capture encoded budget exceeded"));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
