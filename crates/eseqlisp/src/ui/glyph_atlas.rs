//! Cross-platform glyph measurement, rasterization, and atlas packing.
//!
//! Font discovery is provided by `fontdb` and outlines are measured and
//! rasterized by `fontdue` on every platform. Atlas pixels use one explicit
//! convention: row zero is the top row and increasing V moves down. GPU
//! backends must preserve that convention when uploading the R8 bitmap.

use std::collections::HashMap;
use std::sync::OnceLock;

use etagere::{AtlasAllocator, Size};
use fontdb::{Database, Family, Query};
use fontdue::{Font, FontSettings, Metrics};

#[cfg(target_os = "macos")]
use std::ptr::NonNull;
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_metal::{
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture,
    MTLTextureDescriptor,
};

const ATLAS_SIZE: usize = 1024;
const PROP_ATLAS_SIZE: usize = 2048;
const GLYPH_PADDING: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontLineMetrics {
    pub ascent: f32,
    /// Positive distance below the baseline.
    pub descent: f32,
    pub leading: f32,
}

impl FontLineMetrics {
    pub fn line_height(self) -> f32 {
        (self.ascent + self.descent + self.leading).ceil()
    }
}

/// A CPU-owned R8 atlas. Keeping the authoritative bitmap outside a graphics
/// API makes glyph generation testable on headless Linux and lets each backend
/// upload exactly the same pixels.
pub struct AtlasBitmap {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    revision: u64,
}

impl AtlasBitmap {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
            revision: 0,
        }
    }

    fn write(&mut self, x: usize, y: usize, width: usize, height: usize, pixels: &[u8]) {
        debug_assert_eq!(pixels.len(), width * height);
        for row in 0..height {
            let src = row * width;
            let dst = (y + row) * self.width + x;
            self.pixels[dst..dst + width].copy_from_slice(&pixels[src..src + width]);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Top-down, tightly packed R8 coverage pixels.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

struct LoadedFont {
    font: Font,
    post_script_name: String,
}

fn system_fonts() -> &'static Database {
    static SYSTEM_FONTS: OnceLock<Database> = OnceLock::new();
    SYSTEM_FONTS.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    })
}

fn load_font(query: Query<'_>) -> Option<LoadedFont> {
    let db = system_fonts();
    load_font_by_id(db.query(&query)?)
}

fn load_font_by_id(id: fontdb::ID) -> Option<LoadedFont> {
    let db = system_fonts();
    let face = db.face(id)?;
    let post_script_name = face.post_script_name.clone();
    let font = db.with_face_data(id, |data, face_index| {
        Font::from_bytes(
            data.to_vec(),
            FontSettings {
                collection_index: face_index,
                ..FontSettings::default()
            },
        )
        .ok()
    })??;
    Some(LoadedFont {
        font,
        post_script_name,
    })
}

fn load_exact_named_font(name: &str) -> Option<LoadedFont> {
    // The application historically names fonts by PostScript name, while
    // fontdb's family query expects a family name. Try both representations.
    let db = system_fonts();
    let id = db
        .faces()
        .find(|face| face.post_script_name.eq_ignore_ascii_case(name))
        .map(|face| face.id)
        .or_else(|| {
            db.query(&Query {
                families: &[Family::Name(name)],
                ..Query::default()
            })
        })?;
    load_font_by_id(id)
}

fn load_named_font(name: &str) -> Option<LoadedFont> {
    const MONOSPACE_PREFERENCES: &[&str] = &[
        "JetBrains Mono",
        "JetBrains Mono Nerd Font",
        "SF Mono",
        "Menlo",
        "Cascadia Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
    ];

    load_exact_named_font(name)
        .or_else(|| {
            MONOSPACE_PREFERENCES
                .iter()
                .find_map(|name| load_exact_named_font(name))
        })
        .or_else(|| {
            load_font(Query {
                families: &[Family::Monospace],
                ..Query::default()
            })
        })
}

fn load_system_ui_font() -> Option<LoadedFont> {
    #[cfg(target_os = "macos")]
    for name in ["SFPro-Regular", ".AppleSystemUIFont", "Helvetica"] {
        if let Some(font) = load_exact_named_font(name) {
            return Some(font);
        }
    }
    load_font(Query {
        families: &[Family::SansSerif],
        ..Query::default()
    })
}

fn line_metrics(font: &Font, px: f32) -> Option<FontLineMetrics> {
    let metrics = font.horizontal_line_metrics(px)?;
    Some(FontLineMetrics {
        ascent: metrics.ascent,
        descent: -metrics.descent,
        leading: metrics.line_gap,
    })
}

fn copy_glyph_into_line(
    glyph_pixels: &[u8],
    glyph: Metrics,
    destination: &mut [u8],
    destination_w: usize,
    destination_h: usize,
    baseline: f32,
    origin_x: i32,
) {
    if glyph.width == 0 || glyph.height == 0 {
        return;
    }
    // fontdue returns top-down glyph pixels. `ymin` locates the bottom of the
    // glyph relative to the baseline, so this converts to our top-down atlas.
    let dst_x = glyph.xmin - origin_x;
    let dst_y = (baseline - (glyph.ymin + glyph.height as i32) as f32).round() as i32;
    for src_y in 0..glyph.height as i32 {
        let y = dst_y + src_y;
        if !(0..destination_h as i32).contains(&y) {
            continue;
        }
        for src_x in 0..glyph.width as i32 {
            let x = dst_x + src_x;
            if (0..destination_w as i32).contains(&x) {
                destination[y as usize * destination_w + x as usize] =
                    glyph_pixels[src_y as usize * glyph.width + src_x as usize];
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphEntry {
    /// Normalized UVs in top-left, Y-down order.
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

pub struct GlyphAtlas {
    pub cell_w: usize,
    pub cell_h: usize,
    pub ascent: f32,
    pub bitmap: AtlasBitmap,
    allocator: AtlasAllocator,
    glyphs: HashMap<char, GlyphEntry>,
    font: Font,
    font_size: f32,
    descent: f32,
}

impl GlyphAtlas {
    /// Build a headless CPU atlas. GPU backends can upload `bitmap` and track
    /// its revision, while tests can exercise the complete font path on Linux.
    pub fn new(font_name: &str, font_size: f64) -> Option<Self> {
        if !font_size.is_finite() || font_size <= 0.0 {
            return None;
        }
        let loaded = load_named_font(font_name)?;
        if loaded.post_script_name != font_name {
            eprintln!(
                "[glyph_atlas] font family resolution: requested {:?}, got {:?}",
                font_name, loaded.post_script_name
            );
        }
        let font_size = font_size as f32;
        let metrics = line_metrics(&loaded.font, font_size)?;
        let (m_metrics, _) = loaded.font.rasterize('m', font_size);
        let cell_w = m_metrics.advance_width.ceil().max(1.0) as usize;
        let cell_h = metrics.line_height().max(1.0) as usize;
        Some(Self {
            cell_w,
            cell_h,
            ascent: metrics.ascent,
            descent: metrics.descent,
            bitmap: AtlasBitmap::new(ATLAS_SIZE, ATLAS_SIZE),
            allocator: AtlasAllocator::new(Size::new(ATLAS_SIZE as i32, ATLAS_SIZE as i32)),
            glyphs: HashMap::new(),
            font: loaded.font,
            font_size,
        })
    }

    pub fn get_or_rasterize(&mut self, ch: char) -> Option<&GlyphEntry> {
        if !self.glyphs.contains_key(&ch) {
            self.rasterize(ch)?;
        }
        self.glyphs.get(&ch)
    }

    fn rasterize(&mut self, ch: char) -> Option<()> {
        let (glyph_metrics, glyph_pixels) = self.font.rasterize(ch, self.font_size);
        let mut pixels = vec![0; self.cell_w * self.cell_h];
        copy_glyph_into_line(
            &glyph_pixels,
            glyph_metrics,
            &mut pixels,
            self.cell_w,
            self.cell_h,
            self.cell_h as f32 - self.descent,
            0,
        );
        let allocation = self
            .allocator
            .allocate(Size::new(self.cell_w as i32, self.cell_h as i32))?;
        let x = allocation.rectangle.min.x as usize;
        let y = allocation.rectangle.min.y as usize;
        self.bitmap
            .write(x, y, self.cell_w, self.cell_h, &pixels);
        let size = ATLAS_SIZE as f32;
        self.glyphs.insert(
            ch,
            GlyphEntry {
                uv_min: [x as f32 / size, y as f32 / size],
                uv_max: [
                    (x + self.cell_w) as f32 / size,
                    (y + self.cell_h) as f32 / size,
                ],
            },
        );
        Some(())
    }

    pub fn descent(&self) -> f32 {
        self.descent
    }
}

/// Shared scalable system font and cached line metrics for proportional text.
pub struct SizedFontCache {
    font: Font,
    line_metrics: HashMap<u16, FontLineMetrics>,
    scale: f32,
    pub post_script_name: String,
}

impl SizedFontCache {
    pub fn new(scale: f64) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let loaded = load_system_ui_font()?;
        Some(Self {
            font: loaded.font,
            line_metrics: HashMap::new(),
            scale: scale as f32,
            post_script_name: loaded.post_script_name,
        })
    }

    pub fn metrics(&mut self, size_tenths: u16) -> Option<FontLineMetrics> {
        if !self.line_metrics.contains_key(&size_tenths) {
            let px = size_tenths as f32 / 10.0 * self.scale;
            let metrics = line_metrics(&self.font, px)?;
            self.line_metrics.insert(size_tenths, metrics);
        }
        self.line_metrics.get(&size_tenths).copied()
    }

    pub fn line_height(&mut self, size_tenths: u16) -> f32 {
        self.metrics(size_tenths)
            .map(FontLineMetrics::line_height)
            .unwrap_or(0.0)
    }

    pub fn ascent(&mut self, size_tenths: u16) -> f32 {
        self.metrics(size_tenths).map(|m| m.ascent).unwrap_or(0.0)
    }

    pub fn descent(&mut self, size_tenths: u16) -> f32 {
        self.metrics(size_tenths).map(|m| m.descent).unwrap_or(0.0)
    }

    pub fn char_advance(&mut self, ch: char, size_tenths: u16) -> f32 {
        let px = size_tenths as f32 / 10.0 * self.scale;
        self.font.metrics(ch, px).advance_width
    }

    pub fn measure_text(&mut self, text: &str, size_tenths: u16) -> f32 {
        text.chars()
            .map(|ch| self.char_advance(ch, size_tenths))
            .sum()
    }

    fn rasterize(&self, ch: char, size_tenths: u16) -> (Metrics, Vec<u8>) {
        let px = size_tenths as f32 / 10.0 * self.scale;
        self.font.rasterize(ch, px)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProportionalGlyphEntry {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub advance: f32,
    pub raster_w: usize,
    pub raster_h: usize,
    /// Horizontal offset from the pen to the bitmap's left edge.
    pub offset_x: f32,
}

pub struct ProportionalGlyphAtlas {
    pub bitmap: AtlasBitmap,
    allocator: AtlasAllocator,
    glyphs: HashMap<(char, u16), ProportionalGlyphEntry>,
    pub fonts: SizedFontCache,
}

impl ProportionalGlyphAtlas {
    pub fn new(scale: f64) -> Option<Self> {
        Some(Self {
            bitmap: AtlasBitmap::new(PROP_ATLAS_SIZE, PROP_ATLAS_SIZE),
            allocator: AtlasAllocator::new(Size::new(
                PROP_ATLAS_SIZE as i32,
                PROP_ATLAS_SIZE as i32,
            )),
            glyphs: HashMap::new(),
            fonts: SizedFontCache::new(scale)?,
        })
    }

    pub fn line_height(&mut self, size_tenths: u16) -> f32 {
        self.fonts.line_height(size_tenths)
    }

    pub fn ascent(&mut self, size_tenths: u16) -> f32 {
        self.fonts.ascent(size_tenths)
    }

    pub fn descent(&mut self, size_tenths: u16) -> f32 {
        self.fonts.descent(size_tenths)
    }

    pub fn measure_text(&mut self, text: &str, size_tenths: u16) -> f32 {
        self.fonts.measure_text(text, size_tenths)
    }

    pub fn get_or_rasterize(
        &mut self,
        ch: char,
        size_tenths: u16,
    ) -> Option<&ProportionalGlyphEntry> {
        let key = (ch, size_tenths);
        if !self.glyphs.contains_key(&key) {
            self.rasterize(ch, size_tenths)?;
        }
        self.glyphs.get(&key)
    }

    fn rasterize(&mut self, ch: char, size_tenths: u16) -> Option<()> {
        let line_metrics = self.fonts.metrics(size_tenths)?;
        let (glyph, glyph_pixels) = self.fonts.rasterize(ch, size_tenths);
        let advance = glyph.advance_width;
        let raster_h = line_metrics.line_height().max(1.0) as usize;
        let left = glyph.xmin.min(0) - GLYPH_PADDING as i32;
        let right = (glyph.xmin + glyph.width as i32)
            .max(advance.ceil() as i32)
            + GLYPH_PADDING as i32;
        let raster_w = (right - left).max(1) as usize;
        let mut pixels = vec![0; raster_w * raster_h];
        copy_glyph_into_line(
            &glyph_pixels,
            glyph,
            &mut pixels,
            raster_w,
            raster_h,
            raster_h as f32 - line_metrics.descent,
            left,
        );
        let allocation = self
            .allocator
            .allocate(Size::new(raster_w as i32, raster_h as i32))?;
        let x = allocation.rectangle.min.x as usize;
        let y = allocation.rectangle.min.y as usize;
        self.bitmap.write(x, y, raster_w, raster_h, &pixels);
        let size = PROP_ATLAS_SIZE as f32;
        self.glyphs.insert(
            (ch, size_tenths),
            ProportionalGlyphEntry {
                uv_min: [x as f32 / size, y as f32 / size],
                uv_max: [
                    (x + raster_w) as f32 / size,
                    (y + raster_h) as f32 / size,
                ],
                advance,
                raster_w,
                raster_h,
                offset_x: left as f32,
            },
        );
        Some(())
    }
}

#[cfg(target_os = "macos")]
pub struct MetalGlyphAtlas {
    pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
    atlas: GlyphAtlas,
}

#[cfg(target_os = "macos")]
impl MetalGlyphAtlas {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        font_name: &str,
        font_size: f64,
    ) -> Option<Self> {
        Some(Self {
            texture: make_metal_texture(device, ATLAS_SIZE)?,
            atlas: GlyphAtlas::new(font_name, font_size)?,
        })
    }

    pub fn get_or_rasterize(&mut self, ch: char) -> Option<&GlyphEntry> {
        let previous_revision = self.atlas.bitmap.revision();
        let entry = *self.atlas.get_or_rasterize(ch)?;
        if self.atlas.bitmap.revision() != previous_revision {
            upload_entry(
                &self.texture,
                &self.atlas.bitmap,
                entry.uv_min,
                self.atlas.cell_w,
                self.atlas.cell_h,
            );
        }
        self.atlas.glyphs.get(&ch)
    }
}

#[cfg(target_os = "macos")]
impl std::ops::Deref for MetalGlyphAtlas {
    type Target = GlyphAtlas;

    fn deref(&self) -> &Self::Target {
        &self.atlas
    }
}

#[cfg(target_os = "macos")]
impl std::ops::DerefMut for MetalGlyphAtlas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.atlas
    }
}

#[cfg(target_os = "macos")]
pub struct MetalProportionalGlyphAtlas {
    pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
    atlas: ProportionalGlyphAtlas,
}

#[cfg(target_os = "macos")]
impl MetalProportionalGlyphAtlas {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        scale: f64,
    ) -> Option<Self> {
        Some(Self {
            texture: make_metal_texture(device, PROP_ATLAS_SIZE)?,
            atlas: ProportionalGlyphAtlas::new(scale)?,
        })
    }

    pub fn get_or_rasterize(
        &mut self,
        ch: char,
        size_tenths: u16,
    ) -> Option<&ProportionalGlyphEntry> {
        let previous_revision = self.atlas.bitmap.revision();
        let entry = *self.atlas.get_or_rasterize(ch, size_tenths)?;
        if self.atlas.bitmap.revision() != previous_revision {
            upload_entry(
                &self.texture,
                &self.atlas.bitmap,
                entry.uv_min,
                entry.raster_w,
                entry.raster_h,
            );
        }
        self.atlas.glyphs.get(&(ch, size_tenths))
    }
}

#[cfg(target_os = "macos")]
impl std::ops::Deref for MetalProportionalGlyphAtlas {
    type Target = ProportionalGlyphAtlas;

    fn deref(&self) -> &Self::Target {
        &self.atlas
    }
}

#[cfg(target_os = "macos")]
impl std::ops::DerefMut for MetalProportionalGlyphAtlas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.atlas
    }
}

#[cfg(target_os = "macos")]
fn upload_entry(
    texture: &ProtocolObject<dyn MTLTexture>,
    bitmap: &AtlasBitmap,
    uv_min: [f32; 2],
    width: usize,
    height: usize,
) {
    let x = (uv_min[0] * bitmap.width as f32).round() as usize;
    let y = (uv_min[1] * bitmap.height as f32).round() as usize;
    let mut packed = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = (y + row) * bitmap.width + x;
        packed.extend_from_slice(&bitmap.pixels[start..start + width]);
    }
    upload_metal_region(texture, x, y, width, height, &packed);
}

#[cfg(target_os = "macos")]
fn make_metal_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    size: usize,
) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    let descriptor = MTLTextureDescriptor::new();
    unsafe {
        descriptor.setPixelFormat(MTLPixelFormat::R8Unorm);
        descriptor.setWidth(size);
        descriptor.setHeight(size);
    }
    device.newTextureWithDescriptor(&descriptor)
}

#[cfg(target_os = "macos")]
fn upload_metal_region(
    texture: &ProtocolObject<dyn MTLTexture>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    pixels: &[u8],
) {
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            MTLRegion {
                origin: MTLOrigin { x, y, z: 0 },
                size: MTLSize {
                    width,
                    height,
                    depth: 1,
                },
            },
            0,
            NonNull::new(pixels.as_ptr() as *mut core::ffi::c_void).unwrap(),
            width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_monospace_font_name_falls_back_to_an_installed_font() {
        assert!(
            GlyphAtlas::new("ThisFontNameDeliberatelyDoesNotExist", 13.0).is_some(),
            "the monospace atlas should use a system fallback"
        );
    }

    #[test]
    fn cpu_atlases_measure_and_rasterize_on_every_platform() {
        let mut mono = load_font(Query {
            families: &[Family::Monospace],
            ..Query::default()
        })
            .map(|loaded| loaded.post_script_name)
            .and_then(|name| GlyphAtlas::new(&name, 13.0))
            .expect("an installed monospace font");
        let entry = *mono.get_or_rasterize('M').expect("M glyph");
        assert!(mono.cell_w > 0 && mono.cell_h > 0);
        assert!(mono.ascent > 0.0 && mono.descent() >= 0.0);
        assert!(entry.uv_max[0] > entry.uv_min[0]);
        assert!(mono.bitmap.pixels().iter().any(|coverage| *coverage > 0));

        let mut proportional = ProportionalGlyphAtlas::new(1.0)
            .expect("an installed system sans font");
        let narrow = proportional.fonts.char_advance('i', 140);
        let wide = proportional.fonts.char_advance('W', 140);
        assert!(wide > narrow, "system sans font should be proportional");
        let glyph = proportional
            .get_or_rasterize('g', 140)
            .expect("g glyph");
        assert!(glyph.raster_w > 0 && glyph.raster_h > 0);
        assert!(proportional.bitmap.revision() > 0);
    }

    #[test]
    fn atlas_uvs_follow_top_down_bitmap_rows() {
        let loaded = load_font(Query {
            families: &[Family::Monospace],
            ..Query::default()
        })
        .expect("an installed monospace font");
        let mut atlas = GlyphAtlas::new(&loaded.post_script_name, 24.0).unwrap();
        let entry = *atlas.get_or_rasterize('T').unwrap();
        let x0 = (entry.uv_min[0] * ATLAS_SIZE as f32).round() as usize;
        let y0 = (entry.uv_min[1] * ATLAS_SIZE as f32).round() as usize;
        let top_half_coverage: usize = (0..atlas.cell_h / 2)
            .flat_map(|row| {
                &atlas.bitmap.pixels()
                    [(y0 + row) * ATLAS_SIZE + x0..(y0 + row) * ATLAS_SIZE + x0 + atlas.cell_w]
            })
            .map(|v| *v as usize)
            .sum();
        assert!(top_half_coverage > 0, "T must occupy the top half of its cell");
    }
}
