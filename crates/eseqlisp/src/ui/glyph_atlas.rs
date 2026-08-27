//! Glyph measurement, rasterization, and atlas packing.
//!
//! Two font backends sit behind one API. macOS measures and rasterizes with
//! CoreText, so the UI keeps the real system UI face along with Apple's
//! hinting and font smoothing; every other platform discovers fonts with
//! `fontdb` and rasterizes them with `fontdue`. Both fill the same CPU
//! `AtlasBitmap`, so the Metal and wgpu upload paths never learn which
//! backend drew the pixels. Atlas pixels use one explicit convention: row
//! zero is the top row and increasing V moves down. GPU backends must
//! preserve that convention when uploading the R8 bitmap.

use std::collections::HashMap;

use etagere::{AtlasAllocator, Size};

#[cfg(not(target_os = "macos"))]
use fontdb::{Database, Family, Query};
#[cfg(not(target_os = "macos"))]
use fontdue::{Font, FontSettings};
#[cfg(not(target_os = "macos"))]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFRetained, CFString, CGFloat, CGPoint, CGRect, CGSize};
#[cfg(target_os = "macos")]
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGGlyph};
#[cfg(target_os = "macos")]
use objc2_core_text::{CTFont, CTFontOrientation, CTFontUIFontType};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_metal::{
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture,
    MTLTextureDescriptor,
};
#[cfg(target_os = "macos")]
use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::ptr::NonNull;

const ATLAS_SIZE: usize = 1024;
const PROP_ATLAS_SIZE: usize = 2048;
const GLYPH_PADDING: usize = 2;

/// Glyphs whose ink defines the cap band used for vertical centering. Flat-
/// topped capitals only: round ones overshoot the cap line.
const CAP_HEIGHT_REFERENCE_GLYPHS: [char; 2] = ['H', 'X'];

/// Cap height as a fraction of ascent, for fonts that render no reference
/// glyph at all.
const FALLBACK_CAP_HEIGHT_RATIO: f32 = 0.72;

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

/// Raster metrics for one glyph, in device pixels. Mirrors the fields of
/// `fontdue::Metrics` that the atlas needs so the CoreText backend can hand
/// back exactly the same shape.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphMetrics {
    pub width: usize,
    pub height: usize,
    /// Left edge of the ink relative to the pen.
    pub xmin: i32,
    /// Bottom edge of the ink relative to the baseline, positive upward.
    pub ymin: i32,
    pub advance_width: f32,
}

/// Distance from the top of a run's row to the baseline it should sit on,
/// chosen so the font's cap band is optically centered in the row.
///
/// The row is one monospace layout cell tall *at the run's own scale*: a run
/// drawn at `scale` occupies `cell_h * scale` pixels, which is the band every
/// caller reserves for it -- the patcher, the only place `scale` is not 1,
/// centers its text by handing over a row of exactly `1.0 * zoom` cells.
/// Everything here therefore scales together; holding `cell_h` at its
/// unscaled value would peg the text to a fixed height while the box around
/// it grew with zoom.
///
/// Centering the font's *line box* instead of the cap band looks right only
/// when the space above the caps happens to match the descender space below
/// the baseline, which is a coincidence no font owes us. Working from the
/// measured cap band keeps this correct for both backends' metric shapes: a
/// face whose ascent equals its cap height would otherwise ride half a
/// descent high, and half a descent times the zoom in the patcher.
pub fn centered_text_baseline_px(cell_h: f32, cap_height_px: f32, scale: f32) -> f32 {
    (cell_h + cap_height_px) * 0.5 * scale
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

/// A resolved face, plus the name it actually resolved to. `face` is
/// size-independent; every measurement and rasterization names its own pixel
/// size, so one `LoadedFont` serves every zoom level.
struct LoadedFont {
    face: FontFace,
    post_script_name: String,
}

struct NamedFontResolution {
    loaded: LoadedFont,
    used_fallback: bool,
}

// ── fontdb/fontdue backend (every platform except macOS) ──────────────────

/// A parsed outline face. Cloning shares the parsed font.
#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
struct FontFace {
    font: Arc<Font>,
}

#[cfg(not(target_os = "macos"))]
impl FontFace {
    fn line_metrics(&self, px: f32) -> Option<FontLineMetrics> {
        let metrics = self.font.horizontal_line_metrics(px)?;
        Some(FontLineMetrics {
            ascent: metrics.ascent,
            descent: -metrics.descent,
            leading: metrics.line_gap,
        })
    }

    fn metrics(&self, ch: char, px: f32) -> GlyphMetrics {
        convert_metrics(self.font.metrics(ch, px))
    }

    fn advance(&self, ch: char, px: f32) -> f32 {
        self.font.metrics(ch, px).advance_width
    }

    fn rasterize(&self, ch: char, px: f32) -> (GlyphMetrics, Vec<u8>) {
        let (metrics, pixels) = self.font.rasterize(ch, px);
        (convert_metrics(metrics), pixels)
    }
}

#[cfg(not(target_os = "macos"))]
fn convert_metrics(metrics: fontdue::Metrics) -> GlyphMetrics {
    GlyphMetrics {
        width: metrics.width,
        height: metrics.height,
        xmin: metrics.xmin,
        ymin: metrics.ymin,
        advance_width: metrics.advance_width,
    }
}

#[cfg(not(target_os = "macos"))]
fn system_fonts() -> &'static Database {
    static SYSTEM_FONTS: OnceLock<Database> = OnceLock::new();
    SYSTEM_FONTS.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    })
}

#[cfg(not(target_os = "macos"))]
fn load_font(query: Query<'_>) -> Option<LoadedFont> {
    let db = system_fonts();
    load_font_by_id(db.query(&query)?)
}

#[cfg(not(target_os = "macos"))]
fn parsed_fonts() -> &'static Mutex<HashMap<fontdb::ID, Option<Arc<Font>>>> {
    static PARSED_FONTS: OnceLock<Mutex<HashMap<fontdb::ID, Option<Arc<Font>>>>> =
        OnceLock::new();
    PARSED_FONTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_os = "macos"))]
fn load_font_by_id(id: fontdb::ID) -> Option<LoadedFont> {
    let db = system_fonts();
    let face = db.face(id)?;
    let post_script_name = face.post_script_name.clone();
    let mut cache = parsed_fonts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let font = if let Some(cached) = cache.get(&id) {
        cached.clone()?
    } else {
        // Font is immutable and size-independent. Parse each face only once;
        // every atlas size and zoom level can share the resulting object.
        let parsed = db.with_face_data(id, |data, face_index| {
            Font::from_bytes(
                data,
                FontSettings {
                    collection_index: face_index,
                    ..FontSettings::default()
                },
            )
            .ok()
            .map(Arc::new)
        })?;
        cache.insert(id, parsed.clone());
        parsed?
    };
    Some(LoadedFont {
        face: FontFace { font },
        post_script_name,
    })
}

#[cfg(not(target_os = "macos"))]
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

/// Case- and punctuation-insensitive key for comparing font names, so
/// "JetBrainsMono", "JetBrains Mono", and "jetbrains-mono" all compare equal.
fn normalized_font_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// The family part of a PostScript name, which is conventionally
/// `Family-Style`: "JetBrainsMono-Regular" has the stem "jetbrainsmono".
fn font_family_stem(name: &str) -> String {
    normalized_font_key(name.split('-').next().unwrap_or(name))
}

/// Find an installed family whose name starts with `stem`. Distribution builds
/// routinely suffix the family the caller asked for -- a machine carrying
/// "JetBrainsMono Nerd Font" has no plain "JetBrains Mono" -- and that variant
/// is a far better answer than the next entry in a hardcoded preference list.
#[cfg(not(target_os = "macos"))]
fn find_family_by_stem(stem: &str) -> Option<&'static str> {
    if stem.is_empty() {
        return None;
    }
    let db = system_fonts();
    let mut exact: Option<&'static str> = None;
    let mut prefixed: Option<&'static str> = None;
    for face in db.faces() {
        for (family, _) in &face.families {
            let key = normalized_font_key(family);
            if key == stem {
                exact.get_or_insert(family.as_str());
            } else if key.starts_with(stem)
                && prefixed.is_none_or(|current| family.as_str() < current)
            {
                prefixed = Some(family.as_str());
            }
        }
    }
    exact.or(prefixed)
}

#[cfg(not(target_os = "macos"))]
fn load_font_by_family_stem(name: &str) -> Option<LoadedFont> {
    // Query by family so fontdb still picks the upright regular weight within
    // whichever variant family matched.
    let family = find_family_by_stem(&font_family_stem(name))?;
    load_font(Query {
        families: &[Family::Name(family)],
        ..Query::default()
    })
}

/// Last resort: any face at all that fontdue can parse, preferring upright
/// regular weights and, when asked, faces flagged monospaced.
#[cfg(not(target_os = "macos"))]
fn load_any_font(monospaced_only: bool) -> Option<LoadedFont> {
    let db = system_fonts();
    let candidates = |upright: bool| {
        db.faces()
            .filter(move |face| !monospaced_only || face.monospaced)
            .filter(move |face| {
                !upright
                    || (face.style == fontdb::Style::Normal
                        && face.weight == fontdb::Weight::NORMAL)
            })
            .map(|face| face.id)
    };
    candidates(true)
        .find_map(load_font_by_id)
        .or_else(|| candidates(false).find_map(load_font_by_id))
}

#[cfg(not(target_os = "macos"))]
fn load_named_font(name: &str) -> Option<NamedFontResolution> {
    const MONOSPACE_PREFERENCES: &[&str] = &[
        "JetBrains Mono",
        "JetBrains Mono Nerd Font",
        "SF Mono",
        "Menlo",
        "Cascadia Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
    ];

    if let Some(loaded) = load_exact_named_font(name)
        .or_else(|| load_font_by_family_stem(name))
    {
        return Some(NamedFontResolution {
            loaded,
            used_fallback: false,
        });
    }

    let loaded = MONOSPACE_PREFERENCES
        .iter()
        .find_map(|name| load_exact_named_font(name))
        .or_else(|| {
            // fontdb only maps the generic families to real names when
            // fontconfig supplies aliases; elsewhere they stay at the Windows
            // defaults ("Courier New") and can miss on a machine full of fonts.
            load_font(Query {
                families: &[Family::Monospace],
                ..Query::default()
            })
        })
        .or_else(|| load_any_font(true))
        .or_else(|| load_any_font(false))?;
    Some(NamedFontResolution {
        loaded,
        used_fallback: true,
    })
}

#[cfg(target_os = "linux")]
const LINUX_SYSTEM_UI_FONT_PREFERENCES: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Liberation Sans",
    "Cantarell",
    "Inter",
];

#[cfg(not(target_os = "macos"))]
fn load_system_ui_font() -> Option<LoadedFont> {
    #[cfg(target_os = "linux")]
    for name in LINUX_SYSTEM_UI_FONT_PREFERENCES {
        if let Some(font) = load_exact_named_font(name) {
            return Some(font);
        }
    }
    load_font(Query {
        families: &[Family::SansSerif],
        ..Query::default()
    })
    .or_else(|| load_any_font(false))
}

// ── CoreText backend (macOS) ──────────────────────────────────────────────

/// Size a face is created at before anything asks for a specific one. CTFont
/// instances are size-bound, so the base font exists only to be copied at the
/// sizes callers actually request.
#[cfg(target_os = "macos")]
const CORE_TEXT_BASE_SIZE: CGFloat = 16.0;

/// Slack around a glyph's typographic bounds, so antialiased and smoothed
/// edges are not clipped by the ink box CoreText reports.
#[cfg(target_os = "macos")]
const GLYPH_INK_BLEED_PX: i32 = 1;

/// A CoreText face and the sized CTFont instances derived from it. Cloning
/// shares nothing but the base font; the size cache is per-owner and cheap to
/// refill.
#[cfg(target_os = "macos")]
struct FontFace {
    base: CFRetained<CTFont>,
    sized: RefCell<HashMap<u32, CFRetained<CTFont>>>,
}

#[cfg(target_os = "macos")]
impl Clone for FontFace {
    fn clone(&self) -> Self {
        Self::new(self.base.clone())
    }
}

#[cfg(target_os = "macos")]
impl FontFace {
    fn new(base: CFRetained<CTFont>) -> Self {
        Self {
            base,
            sized: RefCell::new(HashMap::new()),
        }
    }

    /// The face at `px`, memoized. Returned by value so no `RefCell` borrow
    /// outlives the lookup.
    fn sized(&self, px: f32) -> CFRetained<CTFont> {
        let key = px.to_bits();
        let mut cache = self.sized.borrow_mut();
        cache
            .entry(key)
            .or_insert_with(|| unsafe {
                self.base
                    .copy_with_attributes(px as CGFloat, std::ptr::null(), None)
            })
            .clone()
    }

    fn line_metrics(&self, px: f32) -> Option<FontLineMetrics> {
        let font = self.sized(px);
        Some(FontLineMetrics {
            ascent: unsafe { font.ascent() } as f32,
            // CTFontGetDescent is already the positive distance below the
            // baseline, which is the convention `FontLineMetrics` uses.
            descent: unsafe { font.descent() } as f32,
            leading: unsafe { font.leading() } as f32,
        })
    }

    fn metrics(&self, ch: char, px: f32) -> GlyphMetrics {
        let font = self.sized(px);
        match glyph_id(&font, ch) {
            Some(glyph) => glyph_metrics(&font, glyph),
            None => GlyphMetrics::default(),
        }
    }

    /// Advance only. Text measurement runs over every label on every layout
    /// pass, so it skips the glyph bounding box it would never look at.
    fn advance(&self, ch: char, px: f32) -> f32 {
        let font = self.sized(px);
        let Some(glyph) = glyph_id(&font, ch) else {
            return 0.0;
        };
        glyph_advance(&font, glyph)
    }

    fn rasterize(&self, ch: char, px: f32) -> (GlyphMetrics, Vec<u8>) {
        let font = self.sized(px);
        let Some(glyph) = glyph_id(&font, ch) else {
            return (GlyphMetrics::default(), Vec::new());
        };
        let tight = glyph_metrics(&font, glyph);
        if tight.width == 0 || tight.height == 0 {
            return (tight, Vec::new());
        }
        let bled = GlyphMetrics {
            width: tight.width + 2 * GLYPH_INK_BLEED_PX as usize,
            height: tight.height + 2 * GLYPH_INK_BLEED_PX as usize,
            xmin: tight.xmin - GLYPH_INK_BLEED_PX,
            ymin: tight.ymin - GLYPH_INK_BLEED_PX,
            ..tight
        };
        match draw_glyph(&font, glyph, bled) {
            Some(pixels) => (bled, pixels),
            None => (
                GlyphMetrics {
                    width: 0,
                    height: 0,
                    ..tight
                },
                Vec::new(),
            ),
        }
    }
}

/// The glyph CoreText maps `ch` to, or `None` when the face cannot draw it.
/// Characters outside the BMP are passed as their surrogate pair, which
/// CTFontGetGlyphsForCharacters resolves into the first output slot.
#[cfg(target_os = "macos")]
fn glyph_id(font: &CTFont, ch: char) -> Option<CGGlyph> {
    let mut utf16 = [0u16; 2];
    let encoded = ch.encode_utf16(&mut utf16);
    let count = encoded.len();
    let mut glyphs = [0 as CGGlyph; 2];
    unsafe {
        font.glyphs_for_characters(
            NonNull::new(utf16.as_mut_ptr()).unwrap(),
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            count as isize,
        );
    }
    (glyphs[0] != 0).then_some(glyphs[0])
}

#[cfg(target_os = "macos")]
fn glyph_advance(font: &CTFont, glyph: CGGlyph) -> f32 {
    let mut glyphs = [glyph];
    let mut advance = CGSize {
        width: 0.0,
        height: 0.0,
    };
    unsafe {
        font.advances_for_glyphs(
            CTFontOrientation::Default,
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            &mut advance,
            1,
        );
    }
    advance.width as f32
}

/// Tight ink box and advance, in the CTFont's own pixel size. The box is
/// snapped outwards to whole pixels so it can address bitmap rows directly.
#[cfg(target_os = "macos")]
fn glyph_metrics(font: &CTFont, glyph: CGGlyph) -> GlyphMetrics {
    let mut glyphs = [glyph];
    let mut bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };
    let mut advance = CGSize {
        width: 0.0,
        height: 0.0,
    };
    unsafe {
        font.bounding_rects_for_glyphs(
            CTFontOrientation::Default,
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            &mut bounds,
            1,
        );
        font.advances_for_glyphs(
            CTFontOrientation::Default,
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            &mut advance,
            1,
        );
    }
    let advance_width = advance.width as f32;
    if !(bounds.size.width > 0.0 && bounds.size.height > 0.0) {
        // Spaces and other blank glyphs still carry an advance.
        return GlyphMetrics {
            advance_width,
            ..GlyphMetrics::default()
        };
    }
    let xmin = bounds.origin.x.floor() as i32;
    let ymin = bounds.origin.y.floor() as i32;
    let xmax = (bounds.origin.x + bounds.size.width).ceil() as i32;
    let ymax = (bounds.origin.y + bounds.size.height).ceil() as i32;
    GlyphMetrics {
        width: (xmax - xmin).max(0) as usize,
        height: (ymax - ymin).max(0) as usize,
        xmin,
        ymin,
        advance_width,
    }
}

/// Rasterize one glyph into a top-down R8 coverage buffer of `metrics`' size.
#[cfg(target_os = "macos")]
fn draw_glyph(font: &CTFont, glyph: CGGlyph, metrics: GlyphMetrics) -> Option<Vec<u8>> {
    let (width, height) = (metrics.width, metrics.height);
    let mut pixels = vec![0u8; width * height];
    {
        let gray = CGColorSpace::new_device_gray()?;
        // kCGImageAlphaNone = 0: one byte per pixel, no alpha. The context
        // draws straight into `pixels` and does not free the buffer.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr() as *mut _,
                width,
                height,
                8,
                width,
                Some(&gray),
                0,
            )
        }?;
        CGContext::set_allows_font_smoothing(Some(&context), true);
        CGContext::set_should_smooth_fonts(Some(&context), true);
        CGContext::set_should_antialias(Some(&context), true);
        // White on the zeroed (black) buffer, so the bytes are coverage.
        let white: [CGFloat; 2] = [1.0, 1.0];
        unsafe { CGContext::set_fill_color(Some(&context), white.as_ptr()) };

        // Place the pen so the ink box lands on the buffer's origin.
        let mut glyphs = [glyph];
        let mut position = CGPoint {
            x: -(metrics.xmin as CGFloat),
            y: -(metrics.ymin as CGFloat),
        };
        unsafe {
            font.draw_glyphs(
                NonNull::new(glyphs.as_mut_ptr()).unwrap(),
                NonNull::new(&mut position).unwrap(),
                1,
                &context,
            );
        }
    }
    // Core Graphics draws with the origin at the lower left but stores the
    // bitmap top row first, which is already the atlas convention.
    Some(pixels)
}

#[cfg(target_os = "macos")]
fn post_script_name(font: &CTFont) -> String {
    unsafe { font.post_script_name() }.to_string()
}

/// CTFontCreateWithName accepts both PostScript and family names and silently
/// substitutes a default face for anything it cannot resolve, so the caller
/// only learns about a fallback by comparing what came back.
#[cfg(target_os = "macos")]
fn load_named_font(name: &str) -> Option<NamedFontResolution> {
    let requested = CFString::from_str(name);
    let font = unsafe { CTFont::with_name(&requested, CORE_TEXT_BASE_SIZE, std::ptr::null()) };
    let resolved = post_script_name(&font);
    let used_fallback = normalized_font_key(&resolved) != normalized_font_key(name)
        && font_family_stem(&resolved) != font_family_stem(name);
    Some(NamedFontResolution {
        loaded: LoadedFont {
            face: FontFace::new(font),
            post_script_name: resolved,
        },
        used_fallback,
    })
}

#[cfg(target_os = "macos")]
fn load_system_ui_font() -> Option<LoadedFont> {
    let font = unsafe {
        CTFont::new_ui_font_for_language(CTFontUIFontType::System, CORE_TEXT_BASE_SIZE, None)
    }?;
    let post_script_name = post_script_name(&font);
    Some(LoadedFont {
        face: FontFace::new(font),
        post_script_name,
    })
}

fn copy_glyph_into_line(
    glyph_pixels: &[u8],
    glyph: GlyphMetrics,
    destination: &mut [u8],
    destination_w: usize,
    destination_h: usize,
    baseline: f32,
    origin_x: i32,
) {
    if glyph.width == 0 || glyph.height == 0 {
        return;
    }
    // Both backends hand back top-down glyph pixels. `ymin` locates the bottom
    // of the glyph relative to the baseline, so this converts to our top-down
    // atlas.
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
    face: FontFace,
    font_size: f32,
    descent: f32,
    leading: f32,
    pub post_script_name: String,
}

impl GlyphAtlas {
    /// Build a headless CPU atlas. GPU backends can upload `bitmap` and track
    /// its revision, while tests can exercise the complete font path on Linux.
    pub fn new(font_name: &str, font_size: f64) -> Option<Self> {
        if !font_size.is_finite() || font_size <= 0.0 {
            return None;
        }
        let resolution = load_named_font(font_name)?;
        let loaded = resolution.loaded;
        if resolution.used_fallback {
            eprintln!(
                "[glyph_atlas] font fallback: requested {:?}, got {:?}",
                font_name, loaded.post_script_name
            );
        }
        let font_size = font_size as f32;
        let metrics = loaded.face.line_metrics(font_size)?;
        let cell_w = loaded.face.advance('m', font_size).ceil().max(1.0) as usize;
        let cell_h = metrics.line_height().max(1.0) as usize;
        Some(Self {
            cell_w,
            cell_h,
            ascent: metrics.ascent,
            descent: metrics.descent,
            bitmap: AtlasBitmap::new(ATLAS_SIZE, ATLAS_SIZE),
            allocator: AtlasAllocator::new(Size::new(ATLAS_SIZE as i32, ATLAS_SIZE as i32)),
            glyphs: HashMap::new(),
            face: loaded.face,
            font_size,
            leading: metrics.leading,
            post_script_name: loaded.post_script_name,
        })
    }

    pub fn get_or_rasterize(&mut self, ch: char) -> Option<&GlyphEntry> {
        if !self.glyphs.contains_key(&ch) {
            self.rasterize(ch)?;
        }
        self.glyphs.get(&ch)
    }

    fn rasterize(&mut self, ch: char) -> Option<()> {
        let (glyph_metrics, glyph_pixels) = self.face.rasterize(ch, self.font_size);
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

    pub fn line_metrics(&self) -> FontLineMetrics {
        FontLineMetrics {
            ascent: self.ascent,
            descent: self.descent,
            leading: self.leading,
        }
    }

    pub fn char_advance(&self, ch: char) -> f32 {
        self.face.advance(ch, self.font_size)
    }
}

/// Shared scalable system font and cached line metrics for proportional text.
pub struct SizedFontCache {
    face: FontFace,
    line_metrics: HashMap<u16, FontLineMetrics>,
    cap_heights: HashMap<u16, f32>,
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
            face: loaded.face,
            line_metrics: HashMap::new(),
            cap_heights: HashMap::new(),
            scale: scale as f32,
            post_script_name: loaded.post_script_name,
        })
    }

    pub fn metrics(&mut self, size_tenths: u16) -> Option<FontLineMetrics> {
        if !self.line_metrics.contains_key(&size_tenths) {
            let px = size_tenths as f32 / 10.0 * self.scale;
            let metrics = self.face.line_metrics(px)?;
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

    /// Rasterized cap height: how far the ink of a capital reaches above the
    /// baseline at this size. Measured rather than read from OS/2 so it
    /// matches the pixels the atlas actually holds, including hinting.
    pub fn cap_height(&mut self, size_tenths: u16) -> f32 {
        if let Some(cap) = self.cap_heights.get(&size_tenths) {
            return *cap;
        }
        let px = size_tenths as f32 / 10.0 * self.scale;
        let measured = CAP_HEIGHT_REFERENCE_GLYPHS
            .iter()
            .map(|ch| {
                let metrics = self.face.metrics(*ch, px);
                (metrics.ymin + metrics.height as i32) as f32
            })
            .fold(0.0_f32, f32::max);
        // A font that cannot draw the reference glyphs still needs a sane
        // band; fall back to the usual cap-height-to-ascent ratio.
        let cap = if measured > 0.0 {
            measured
        } else {
            self.ascent(size_tenths) * FALLBACK_CAP_HEIGHT_RATIO
        };
        self.cap_heights.insert(size_tenths, cap);
        cap
    }

    pub fn char_advance(&mut self, ch: char, size_tenths: u16) -> f32 {
        let px = size_tenths as f32 / 10.0 * self.scale;
        self.face.advance(ch, px)
    }

    pub fn measure_text(&mut self, text: &str, size_tenths: u16) -> f32 {
        text.chars()
            .map(|ch| self.char_advance(ch, size_tenths))
            .sum()
    }

    fn rasterize(&self, ch: char, size_tenths: u16) -> (GlyphMetrics, Vec<u8>) {
        let px = size_tenths as f32 / 10.0 * self.scale;
        self.face.rasterize(ch, px)
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

    pub fn cap_height(&mut self, size_tenths: u16) -> f32 {
        self.fonts.cap_height(size_tenths)
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

    /// PostScript name of some monospace face this machine really has, so the
    /// backend-neutral tests below do not depend on either backend's discovery.
    #[cfg(target_os = "macos")]
    fn installed_monospace_font_name() -> String {
        // Bundled with every macOS install.
        "Menlo-Regular".to_string()
    }

    #[cfg(not(target_os = "macos"))]
    fn installed_monospace_font_name() -> String {
        // Generic-family aliases are not guaranteed to be configured even
        // when fontdb loaded real monospaced faces (as on the minimal Linux CI
        // image). Select from the faces themselves, matching the production
        // fallback path rather than relying on a fontconfig alias.
        load_any_font(true)
            .map(|loaded| loaded.post_script_name)
            .expect("an installed monospace font")
    }

    #[test]
    fn unknown_monospace_font_name_falls_back_to_an_installed_font() {
        let name = "ThisFontNameDeliberatelyDoesNotExist";
        let resolution = load_named_font(name).expect("the fallback chain must find a font");
        assert!(resolution.used_fallback);
        assert!(
            GlyphAtlas::new(name, 13.0).is_some(),
            "the monospace atlas should use a system fallback"
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn the_last_resort_fallback_ignores_every_configured_family_name() {
        // The tail of the chain must not depend on fontconfig aliases or on the
        // preference list, so text still renders on a machine whose fonts are
        // all named something we have never heard of.
        assert!(
            load_any_font(false).is_some(),
            "a machine with fonts must always yield some face"
        );
    }

    #[test]
    fn font_name_matching_ignores_case_punctuation_and_style_suffixes() {
        assert_eq!(normalized_font_key("JetBrains Mono"), "jetbrainsmono");
        assert_eq!(normalized_font_key("jetbrains-mono"), "jetbrainsmono");
        assert_eq!(font_family_stem("JetBrainsMono-Regular"), "jetbrainsmono");
        assert_eq!(font_family_stem("SFPro-Regular"), "sfpro");
        assert_eq!(font_family_stem(""), "");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_postscript_name_resolves_to_a_suffixed_family_variant() {
        // Machine-independent: derive a PostScript-style request from a family
        // that is actually installed here, then require the stem lookup to find
        // that family (or a variant of it) rather than falling through to an
        // unrelated preference-list font.
        let family = system_fonts()
            .faces()
            .find_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .expect("the test machine must have at least one font installed");
        let request = format!("{}-Regular", family.replace(' ', ""));
        let stem = font_family_stem(&request);
        let matched = find_family_by_stem(&stem)
            .unwrap_or_else(|| panic!("no family matched the stem of {request:?}"));
        assert!(
            normalized_font_key(matched).starts_with(&stem),
            "{matched:?} does not start with the requested stem {stem:?}"
        );
        assert!(
            load_font_by_family_stem(&request).is_some(),
            "the stem match must produce a loadable face"
        );
        let resolution = load_named_font(&request)
            .expect("the family-name resolution must produce a loadable face");
        assert!(
            !resolution.used_fallback,
            "a requested family variant is a resolution, not a fallback"
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn an_exact_name_still_wins_over_every_fallback() {
        let expected = system_fonts()
            .faces()
            .find(|face| face.style == fontdb::Style::Normal)
            .map(|face| face.post_script_name.clone())
            .expect("the test machine must have at least one font installed");
        let resolution = load_named_font(&expected).expect("an installed font must load by name");
        assert!(!resolution.used_fallback);
        assert_eq!(resolution.loaded.post_script_name, expected);
    }

    #[test]
    // The CoreText backend derives every size from one base CTFont instead.
    #[cfg(not(target_os = "macos"))]
    fn atlas_rebuilds_at_new_sizes_reuse_the_parsed_font() {
        let expected = installed_monospace_font_name();
        let small = GlyphAtlas::new(&expected, 12.0).expect("small atlas");
        let large = GlyphAtlas::new(&expected, 24.0).expect("large atlas");

        assert!(
            Arc::ptr_eq(&small.face.font, &large.face.font),
            "atlas size changes must not reparse the selected font"
        );
    }

    /// The UI font must be the real system face CoreText resolves, not a
    /// substitute: the fontdue port could not rasterize SFNS and pinned
    /// Helvetica instead, which changed every proportional label on macOS.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_macos_ui_font_is_the_core_text_system_face() {
        let loaded = load_system_ui_font().expect("the macOS system UI font");
        let name = normalized_font_key(&loaded.post_script_name);
        assert!(
            name.contains("sfns") || name.contains("sfpro") || name.contains("systemui"),
            "expected the San Francisco system UI face, got {:?}",
            loaded.post_script_name
        );

        let (metrics, pixels) = loaded.face.rasterize('H', 20.0);
        assert!(
            metrics.width > 0 && metrics.height > 0 && pixels.iter().any(|pixel| *pixel != 0),
            "{:?} must produce visible glyphs through CoreText",
            loaded.post_script_name
        );
    }

    /// CoreText's JetBrains Mono cell at 16pt, as it was before the fontdue
    /// port.
    #[cfg(target_os = "macos")]
    const JETBRAINS_MONO_16PT_CELL: (usize, usize) = (10, 22);

    /// The monospace cell is the one number every layout in the app is built
    /// on. These are the CoreText values from before the fontdue port; a
    /// change here silently reflows the entire UI.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_jetbrains_mono_cell_matches_the_pre_port_core_text_metrics() {
        let atlas = GlyphAtlas::new("JetBrainsMono-Regular", 16.0)
            .expect("JetBrains Mono is the application's monospace font");
        assert_eq!(atlas.post_script_name, "JetBrainsMono-Regular");
        assert_eq!((atlas.cell_w, atlas.cell_h), JETBRAINS_MONO_16PT_CELL);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn system_ui_font_is_not_a_condensed_face_on_linux() {
        let loaded = load_system_ui_font().expect("an installed system UI font");
        let face = system_fonts()
            .faces()
            .find(|face| face.post_script_name == loaded.post_script_name)
            .expect("the loaded font must remain in the system font database");
        let names = face
            .families
            .iter()
            .fold(face.post_script_name.clone(), |mut names, (family, _)| {
                names.push_str(family);
                names
            });
        let normalized_names = normalized_font_key(&names);

        assert!(
            face.stretch >= fontdb::Stretch::Normal
                && !normalized_names.contains("condensed")
                && !normalized_names.contains("narrow"),
            "Linux system UI font must not be condensed: name={:?} families={:?} stretch={:?}",
            face.post_script_name,
            face.families,
            face.stretch
        );
    }

    /// Ink center of a capital, in device pixels from the top of the mono cell
    /// it is centered in — mirrors what `build_proportional_text_quads` does.
    fn centered_ink_center_px(
        atlas: &mut ProportionalGlyphAtlas,
        ch: char,
        size_tenths: u16,
        cell_h: f32,
        scale: f32,
    ) -> f32 {
        let entry = *atlas.get_or_rasterize(ch, size_tenths).expect("glyph");
        let line_height = atlas.line_height(size_tenths);
        let descent = atlas.descent(size_tenths);
        let cap = atlas.cap_height(size_tenths);
        let y_offset =
            centered_text_baseline_px(cell_h, cap, scale) - (line_height - descent) * scale;

        let atlas_w = atlas.bitmap.width();
        let x0 = (entry.uv_min[0] * atlas_w as f32).round() as usize;
        let y0 = (entry.uv_min[1] * atlas.bitmap.height() as f32).round() as usize;
        let pixels = atlas.bitmap.pixels();
        let inked = |row: usize| {
            (0..entry.raster_w).any(|col| pixels[(y0 + row) * atlas_w + x0 + col] > 8)
        };
        let top = (0..entry.raster_h).find(|row| inked(*row)).expect("ink");
        let bottom = (0..entry.raster_h)
            .rev()
            .find(|row| inked(*row))
            .expect("ink")
            + 1;
        y_offset + (top + bottom) as f32 / 2.0 * scale
    }

    #[test]
    fn capitals_center_vertically_in_their_layout_cell() {
        let mut atlas = ProportionalGlyphAtlas::new(2.0).expect("proportional atlas");

        for size_tenths in [100_u16, 120, 140] {
            for cell_h in [20.0_f32, 26.0, 31.0] {
                for ch in ['H', 'S', 'R', '1'] {
                    let ink_center =
                        centered_ink_center_px(&mut atlas, ch, size_tenths, cell_h, 1.0);
                    let offset = ink_center - cell_h * 0.5;
                    // Round capitals overshoot the cap line slightly and the
                    // rasterizer rounds to whole pixels; a full pixel of slack
                    // at 2x is a quarter of what line-box centering drifted by.
                    assert!(
                        offset.abs() <= 1.0,
                        "'{ch}' at {size_tenths} tenths in a {cell_h}px cell is off center \
                         by {offset:+.2}px (font {})",
                        atlas.fonts.post_script_name
                    );
                }
            }
        }
    }

    /// A scaled run — the patcher at any zoom other than 1 — is centered in a
    /// row that scaled with it, not in a fixed one-cell row.
    #[test]
    fn scaled_runs_center_in_a_row_that_scaled_with_them() {
        let mut atlas = ProportionalGlyphAtlas::new(2.0).expect("proportional atlas");
        let cell_h = 26.0_f32;

        for scale in [0.35_f32, 0.7, 1.0, 1.8, 2.5] {
            for ch in ['H', 'R'] {
                let ink_center = centered_ink_center_px(&mut atlas, ch, 160, cell_h, scale);
                let offset = ink_center - cell_h * scale * 0.5;
                assert!(
                    offset.abs() <= 1.0 * scale.max(1.0),
                    "'{ch}' at zoom {scale} is off the center of its {:.1}px row by \
                     {offset:+.2}px (font {})",
                    cell_h * scale,
                    atlas.fonts.post_script_name
                );
            }
        }
    }

    #[test]
    fn cpu_atlases_measure_and_rasterize_on_every_platform() {
        let mut mono = GlyphAtlas::new(&installed_monospace_font_name(), 13.0)
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
        let mut atlas = GlyphAtlas::new(&installed_monospace_font_name(), 24.0).unwrap();
        let entry = *atlas.get_or_rasterize('T').unwrap();
        let x0 = (entry.uv_min[0] * ATLAS_SIZE as f32).round() as usize;
        let y0 = (entry.uv_min[1] * ATLAS_SIZE as f32).round() as usize;
        let coverage = |rows: std::ops::Range<usize>| -> usize {
            rows.flat_map(|row| {
                &atlas.bitmap.pixels()
                    [(y0 + row) * ATLAS_SIZE + x0..(y0 + row) * ATLAS_SIZE + x0 + atlas.cell_w]
            })
            .map(|v| *v as usize)
            .sum()
        };
        // A 'T' is bar-heavy at the top and a bare stem below, so a backend
        // that hands back bottom-up rows inverts this comparison.
        let top = coverage(0..atlas.cell_h / 2);
        let bottom = coverage(atlas.cell_h / 2..atlas.cell_h);
        assert!(
            top > bottom && bottom > 0,
            "T should be top-heavy in a top-down cell, got top={top} bottom={bottom}"
        );
    }
}
