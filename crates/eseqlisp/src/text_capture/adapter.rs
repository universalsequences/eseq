use super::{FontMeasurement, GlyphRaster, MONOSPACE_FONT_NAME, TextCaptureSource};
use crate::glyph_atlas::{GlyphAtlas, ProportionalGlyphAtlas, SizedFontCache};

pub struct PlatformTextCaptureSource;

impl PlatformTextCaptureSource {
    fn mono_atlas(font_size: f32, scale_factor: f32) -> Result<GlyphAtlas, String> {
        GlyphAtlas::new(MONOSPACE_FONT_NAME, f64::from(font_size * scale_factor))
            .ok_or_else(|| "could not load a monospace font".to_string())
    }

    fn proportional_atlas(scale_factor: f32) -> Result<ProportionalGlyphAtlas, String> {
        ProportionalGlyphAtlas::new(f64::from(scale_factor))
            .ok_or_else(|| "could not load the system UI font".to_string())
    }
}

impl TextCaptureSource for PlatformTextCaptureSource {
    fn backend_name(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "coretext"
        } else {
            "fontdue"
        }
    }

    fn mono_font_name(&self) -> Result<String, String> {
        Ok(Self::mono_atlas(16.0, 1.0)?.post_script_name.clone())
    }

    fn proportional_font_name(&self) -> Result<String, String> {
        Ok(SizedFontCache::new(1.0)
            .ok_or_else(|| "could not load the system UI font".to_string())?
            .post_script_name)
    }

    fn measure_mono(
        &self,
        font_size: f32,
        scale_factor: f32,
        charset: &str,
    ) -> Result<FontMeasurement, String> {
        let atlas = Self::mono_atlas(font_size, scale_factor)?;
        let metrics = atlas.line_metrics();
        Ok(FontMeasurement {
            cell_w: atlas.cell_w as f32,
            cell_h: atlas.cell_h as f32,
            ascent: metrics.ascent,
            descent: metrics.descent,
            leading: metrics.leading,
            advance_widths: charset
                .chars()
                .map(|ch| (ch, atlas.char_advance(ch)))
                .collect(),
        })
    }

    fn measure_proportional(
        &self,
        font_size: f32,
        scale_factor: f32,
        charset: &str,
    ) -> Result<FontMeasurement, String> {
        let mut fonts = SizedFontCache::new(f64::from(scale_factor))
            .ok_or_else(|| "could not load the system UI font".to_string())?;
        let size_tenths = size_tenths(font_size)?;
        let metrics = fonts
            .metrics(size_tenths)
            .ok_or_else(|| format!("no line metrics for {font_size}"))?;
        Ok(FontMeasurement {
            cell_w: fonts.char_advance('m', size_tenths),
            cell_h: metrics.line_height(),
            ascent: metrics.ascent,
            descent: metrics.descent,
            leading: metrics.leading,
            advance_widths: charset
                .chars()
                .map(|ch| (ch, fonts.char_advance(ch, size_tenths)))
                .collect(),
        })
    }

    fn rasterize_mono_text(
        &self,
        text: &str,
        font_size: f32,
        scale_factor: f32,
    ) -> Result<Vec<GlyphRaster>, String> {
        let mut atlas = Self::mono_atlas(font_size, scale_factor)?;
        let mut entries = Vec::with_capacity(text.chars().count());
        for ch in text.chars() {
            let entry = *atlas
                .get_or_rasterize(ch)
                .ok_or_else(|| format!("could not rasterize {ch:?}"))?;
            entries.push((entry, atlas.cell_w, atlas.cell_h));
        }
        Ok(entries
            .into_iter()
            .map(|(entry, width, height)| {
                extract_raster(
                    &atlas.bitmap,
                    entry.uv_min,
                    width,
                    height,
                    0.0,
                    width as f32,
                )
            })
            .collect())
    }

    fn rasterize_proportional_text(
        &self,
        text: &str,
        font_size: f32,
        scale_factor: f32,
    ) -> Result<Vec<GlyphRaster>, String> {
        let mut atlas = Self::proportional_atlas(scale_factor)?;
        let size_tenths = size_tenths(font_size)?;
        let mut entries = Vec::with_capacity(text.chars().count());
        for ch in text.chars() {
            entries.push(
                *atlas
                    .get_or_rasterize(ch, size_tenths)
                    .ok_or_else(|| format!("could not rasterize {ch:?}"))?,
            );
        }
        Ok(entries
            .into_iter()
            .map(|entry| {
                extract_raster(
                    &atlas.bitmap,
                    entry.uv_min,
                    entry.raster_w,
                    entry.raster_h,
                    entry.offset_x,
                    entry.advance,
                )
            })
            .collect())
    }
}

fn size_tenths(font_size: f32) -> Result<u16, String> {
    if !font_size.is_finite() || font_size <= 0.0 || font_size > u16::MAX as f32 / 10.0 {
        return Err(format!("invalid font size {font_size}"));
    }
    Ok((font_size * 10.0).round() as u16)
}

fn extract_raster(
    bitmap: &crate::glyph_atlas::AtlasBitmap,
    uv_min: [f32; 2],
    width: usize,
    height: usize,
    offset_x: f32,
    advance: f32,
) -> GlyphRaster {
    let x = (uv_min[0] * bitmap.width() as f32).round() as usize;
    let y = (uv_min[1] * bitmap.height() as f32).round() as usize;
    let mut pixels = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = (y + row) * bitmap.width() + x;
        pixels.extend_from_slice(&bitmap.pixels()[start..start + width]);
    }
    GlyphRaster {
        width,
        height,
        offset_x,
        advance,
        pixels,
    }
}
