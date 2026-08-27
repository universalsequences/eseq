//! Stable metrics and raster-image contract for comparing text backends.
//!
//! The contract and PNG compositor deliberately do not depend on fontdue,
//! CoreText, or a graphics API. A historical text backend only needs to
//! implement [`TextCaptureSource`] to produce the same artifacts.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

pub const SCHEMA_VERSION: u32 = 1;
pub const MONOSPACE_FONT_NAME: &str = "JetBrainsMono-Regular";
pub const MONOSPACE_FONT_SIZES: &[f32] = &[16.0];
pub const PROPORTIONAL_FONT_SIZES: &[f32] = &[
    6.5, 9.0, 9.5, 10.0, 10.5, 11.0, 11.5, 12.0, 13.0, 14.0, 15.0, 16.0,
];
pub const SCALE_FACTORS: &[f32] = &[1.0, 1.5, 2.0];
pub const PRINTABLE_ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";
pub const CAPTURE_WIDTH: u32 = 960;
pub const CAPTURE_HEIGHT: u32 = 360;
pub const FIXED_BUFFER: &str = include_str!("../tests/fixtures/text-capture/buffer.txt");

#[derive(Clone, Debug)]
pub struct FontMeasurement {
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub advance_widths: Vec<(char, f32)>,
}

#[derive(Clone, Debug)]
pub struct GlyphRaster {
    pub width: usize,
    pub height: usize,
    /// Horizontal offset from the pen position to the bitmap's left edge.
    pub offset_x: f32,
    pub advance: f32,
    /// Top-down, tightly packed coverage bytes.
    pub pixels: Vec<u8>,
}

pub trait TextCaptureSource {
    fn backend_name(&self) -> &'static str;
    fn mono_font_name(&self) -> Result<String, String>;
    fn proportional_font_name(&self) -> Result<String, String>;
    fn measure_mono(
        &self,
        font_size: f32,
        scale_factor: f32,
        charset: &str,
    ) -> Result<FontMeasurement, String>;
    fn measure_proportional(
        &self,
        font_size: f32,
        scale_factor: f32,
        charset: &str,
    ) -> Result<FontMeasurement, String>;
    fn rasterize_mono_text(
        &self,
        text: &str,
        font_size: f32,
        scale_factor: f32,
    ) -> Result<Vec<GlyphRaster>, String>;
    fn rasterize_proportional_text(
        &self,
        text: &str,
        font_size: f32,
        scale_factor: f32,
    ) -> Result<Vec<GlyphRaster>, String>;
}

mod adapter;
pub use adapter::PlatformTextCaptureSource;

#[derive(Clone, Debug)]
pub struct CapturePaths {
    pub metrics: PathBuf,
    pub screenshot: PathBuf,
}

pub fn capture(
    source: &impl TextCaptureSource,
    output_root: &Path,
    capture_name: &str,
) -> Result<CapturePaths, String> {
    validate_capture_name(capture_name)?;
    let output_dir = output_root.join(capture_name);
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("could not create {}: {error}", output_dir.display()))?;

    let metrics = output_dir.join("metrics.json");
    let screenshot = output_dir.join("text.png");
    let json = metrics_json(source, capture_name)?;
    fs::write(&metrics, json)
        .map_err(|error| format!("could not write {}: {error}", metrics.display()))?;
    render_screenshot(source, &screenshot)?;
    Ok(CapturePaths {
        metrics,
        screenshot,
    })
}

fn validate_capture_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("capture name must contain only ASCII letters, digits, '-' or '_'".to_string());
    }
    Ok(())
}

fn metrics_json(source: &impl TextCaptureSource, capture_name: &str) -> Result<String, String> {
    let mono_font = source.mono_font_name()?;
    let proportional_font = source.proportional_font_name()?;
    let mut output = String::new();
    writeln!(output, "{{").unwrap();
    writeln!(output, "  \"schema_version\": {SCHEMA_VERSION},").unwrap();
    writeln!(
        output,
        "  \"capture_name\": {},",
        serde_json::to_string(capture_name).unwrap()
    )
    .unwrap();
    writeln!(
        output,
        "  \"backend\": {},",
        serde_json::to_string(source.backend_name()).unwrap()
    )
    .unwrap();
    writeln!(
        output,
        "  \"monospace_font\": {},",
        serde_json::to_string(&mono_font).unwrap()
    )
    .unwrap();
    writeln!(
        output,
        "  \"proportional_font\": {},",
        serde_json::to_string(&proportional_font).unwrap()
    )
    .unwrap();
    writeln!(
        output,
        "  \"charset\": {},",
        serde_json::to_string(PRINTABLE_ASCII).unwrap()
    )
    .unwrap();
    writeln!(output, "  \"measurements\": [").unwrap();

    let total = (MONOSPACE_FONT_SIZES.len() + PROPORTIONAL_FONT_SIZES.len()) * SCALE_FACTORS.len();
    let mut index = 0;
    for (kind, sizes) in [
        ("monospace", MONOSPACE_FONT_SIZES),
        ("proportional", PROPORTIONAL_FONT_SIZES),
    ] {
        for &font_size in sizes {
            for &scale_factor in SCALE_FACTORS {
                let measurement = if kind == "monospace" {
                    source.measure_mono(font_size, scale_factor, PRINTABLE_ASCII)?
                } else {
                    source.measure_proportional(font_size, scale_factor, PRINTABLE_ASCII)?
                };
                write_measurement(
                    &mut output,
                    kind,
                    font_size,
                    scale_factor,
                    &measurement,
                    index + 1 == total,
                );
                index += 1;
            }
        }
    }
    writeln!(output, "  ]").unwrap();
    writeln!(output, "}}").unwrap();
    Ok(output)
}

fn write_measurement(
    output: &mut String,
    kind: &str,
    font_size: f32,
    scale_factor: f32,
    measurement: &FontMeasurement,
    last: bool,
) {
    writeln!(output, "    {{").unwrap();
    writeln!(output, "      \"kind\": \"{kind}\",").unwrap();
    writeln!(output, "      \"font_size\": {font_size:.4},").unwrap();
    writeln!(output, "      \"scale_factor\": {scale_factor:.4},").unwrap();
    writeln!(output, "      \"cell_w\": {:.4},", measurement.cell_w).unwrap();
    writeln!(output, "      \"cell_h\": {:.4},", measurement.cell_h).unwrap();
    writeln!(output, "      \"ascent\": {:.4},", measurement.ascent).unwrap();
    writeln!(output, "      \"descent\": {:.4},", measurement.descent).unwrap();
    writeln!(output, "      \"leading\": {:.4},", measurement.leading).unwrap();
    writeln!(output, "      \"advance_widths\": {{").unwrap();
    for (index, (ch, advance)) in measurement.advance_widths.iter().enumerate() {
        let comma = if index + 1 == measurement.advance_widths.len() {
            ""
        } else {
            ","
        };
        writeln!(
            output,
            "        {}: {advance:.4}{comma}",
            serde_json::to_string(&ch.to_string()).unwrap()
        )
        .unwrap();
    }
    writeln!(output, "      }}").unwrap();
    writeln!(output, "    }}{}", if last { "" } else { "," }).unwrap();
}

fn render_screenshot(source: &impl TextCaptureSource, path: &Path) -> Result<(), String> {
    let mut canvas = Canvas::new(CAPTURE_WIDTH, CAPTURE_HEIGHT, [15, 19, 26, 255]);
    canvas.fill_rect(24, 22, CAPTURE_WIDTH - 48, 2, [70, 190, 180, 255]);
    let mut y = 42_i32;
    for (line_index, line) in FIXED_BUFFER.lines().enumerate() {
        let (proportional, size, color) = match line_index {
            0 => (true, 16.0, [230, 237, 243, 255]),
            1 => (true, 12.0, [145, 155, 170, 255]),
            _ => (false, 16.0, [207, 216, 220, 255]),
        };
        draw_text(
            source,
            &mut canvas,
            line,
            32.0,
            y,
            size,
            proportional,
            color,
        )?;
        y += if proportional { 38 } else { 30 };
    }
    canvas.write_png(path)
}

fn draw_text(
    source: &impl TextCaptureSource,
    canvas: &mut Canvas,
    text: &str,
    start_x: f32,
    y: i32,
    font_size: f32,
    proportional: bool,
    color: [u8; 4],
) -> Result<(), String> {
    let glyphs = if proportional {
        source.rasterize_proportional_text(text, font_size, 1.0)?
    } else {
        source.rasterize_mono_text(text, font_size, 1.0)?
    };
    let mut pen_x = start_x;
    for glyph in glyphs {
        canvas.blend_mask(
            (pen_x + glyph.offset_x).round() as i32,
            y,
            glyph.width,
            glyph.height,
            &glyph.pixels,
            color,
        );
        pen_x += glyph.advance;
    }
    Ok(())
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, color: [u8; 4]) -> Self {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            pixels.extend_from_slice(&color);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    fn fill_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
        for row in y..(y + height).min(self.height) {
            for col in x..(x + width).min(self.width) {
                let offset = (row * self.width + col) as usize * 4;
                self.pixels[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }

    fn blend_mask(
        &mut self,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        mask: &[u8],
        color: [u8; 4],
    ) {
        debug_assert_eq!(mask.len(), width * height);
        for source_y in 0..height {
            let destination_y = y + source_y as i32;
            if !(0..self.height as i32).contains(&destination_y) {
                continue;
            }
            for source_x in 0..width {
                let destination_x = x + source_x as i32;
                if !(0..self.width as i32).contains(&destination_x) {
                    continue;
                }
                let coverage = mask[source_y * width + source_x] as u16;
                if coverage == 0 {
                    continue;
                }
                let offset =
                    (destination_y as u32 * self.width + destination_x as u32) as usize * 4;
                for channel in 0..3 {
                    let background = self.pixels[offset + channel] as u16;
                    let foreground = color[channel] as u16;
                    self.pixels[offset + channel] =
                        ((foreground * coverage + background * (255 - coverage) + 127) / 255) as u8;
                }
                self.pixels[offset + 3] = 255;
            }
        }
    }

    fn write_png(&self, path: &Path) -> Result<(), String> {
        let file = fs::File::create(path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        PngEncoder::new_with_quality(file, CompressionType::Best, FilterType::Adaptive)
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                ExtendedColorType::Rgba8,
            )
            .map_err(|error| format!("could not encode {}: {error}", path.display()))
    }
}
