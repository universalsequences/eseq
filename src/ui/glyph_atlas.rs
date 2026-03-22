/// Glyph atlas: rasterizes characters via CoreText into a shared R8Unorm Metal
/// texture. Each cell is `cell_w × cell_h` pixels; the atlas packs them with
/// etagere. The shader samples coverage (0=transparent, 1=opaque) and blends
/// the caller's fg/bg colors — so the atlas is colorless.
///
/// Coordinate note: CoreText draws Y-up into the CGBitmapContext, but Metal
/// textures are Y-down. Rather than flipping pixels on the CPU, we flip UV.v
/// in the vertex shader (v = 1.0 - v).
#[cfg(target_os = "macos")]
mod inner {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::ptr::NonNull;

    use etagere::{AtlasAllocator, Size};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_core_foundation::{CFRetained, CFString, CGFloat, CGPoint, CGSize};
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGGlyph};
    use objc2_core_text::{CTFont, CTFontOrientation, CTFontUIFontType};
    use objc2_metal::{
        MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture, MTLTextureDescriptor,
    };

    const ATLAS_SIZE: usize = 1024;
    // kCFStringEncodingUTF8
    const CF_UTF8: u32 = 0x0800_0100;

    /// Per-character entry in the atlas.
    #[derive(Clone, Copy, Debug)]
    pub struct GlyphEntry {
        /// Normalised UV corners (0..1) in the atlas texture.
        /// v is in CoreText Y-up order; flip in the shader (v = 1.0 - v).
        pub uv_min: [f32; 2],
        pub uv_max: [f32; 2],
    }

    pub struct GlyphAtlas {
        /// R8Unorm GPU texture, ATLAS_SIZE × ATLAS_SIZE.
        pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
        /// Advance width in pixels (monospace — same for every glyph).
        pub cell_w: usize,
        /// Line height in pixels (ascent + descent + leading).
        pub cell_h: usize,
        /// Pixels above the baseline (used to position each cell row).
        pub ascent: f32,

        allocator: AtlasAllocator,
        glyphs: HashMap<char, GlyphEntry>,
        font: CFRetained<CTFont>,
        descent: f32,
    }

    impl GlyphAtlas {
        pub fn new(
            device: &ProtocolObject<dyn MTLDevice>,
            font_name: &str,
            font_size: f64,
        ) -> Option<Self> {
            // ── CTFont ───────────────────────────────────────────────────────────
            let cf_name = {
                let cstr = CString::new(font_name).ok()?;
                unsafe { CFString::with_c_string(None, cstr.as_ptr(), CF_UTF8) }?
            };
            let font = unsafe { CTFont::with_name(&cf_name, font_size, std::ptr::null()) };

            // Warn if CoreText fell back to a different font (e.g. bad PostScript name).
            let resolved = unsafe { font.post_script_name() };
            let resolved_str = resolved.to_string();
            if resolved_str != font_name {
                eprintln!(
                    "[glyph_atlas] font fallback: requested {:?}, got {:?}",
                    font_name, resolved_str
                );
            }

            // ── Cell dimensions from font metrics ────────────────────────────────
            let ascent = unsafe { font.ascent() } as f32;
            let descent = unsafe { font.descent() } as f32; // positive value
            let leading = unsafe { font.leading() } as f32;
            let cell_h = (ascent + descent + leading).ceil() as usize;

            // Advance width: measure 'm' as the monospace cell width.
            let cell_w = {
                let chars: [u16; 1] = ['m' as u16];
                let mut glyph: [CGGlyph; 1] = [0];
                unsafe {
                    font.glyphs_for_characters(
                        NonNull::new(chars.as_ptr() as *mut _).unwrap(),
                        NonNull::new(glyph.as_mut_ptr()).unwrap(),
                        1,
                    );
                }
                let mut adv = CGSize {
                    width: 0.0,
                    height: 0.0,
                };
                unsafe {
                    font.advances_for_glyphs(
                        CTFontOrientation::Default,
                        NonNull::new(glyph.as_mut_ptr()).unwrap(),
                        &mut adv,
                        1,
                    );
                }
                adv.width.ceil() as usize
            };

            // ── Atlas GPU texture (R8Unorm) ──────────────────────────────────────
            let texture = {
                let desc = MTLTextureDescriptor::new();
                unsafe {
                    desc.setPixelFormat(MTLPixelFormat::R8Unorm);
                    desc.setWidth(ATLAS_SIZE);
                    desc.setHeight(ATLAS_SIZE);
                }
                device.newTextureWithDescriptor(&desc)?
            };

            Some(Self {
                texture,
                cell_w,
                cell_h,
                ascent,
                descent,
                allocator: AtlasAllocator::new(Size::new(ATLAS_SIZE as i32, ATLAS_SIZE as i32)),
                glyphs: HashMap::new(),
                font,
            })
        }

        /// Return the atlas entry for `ch`, rasterizing it on first access.
        pub fn get_or_rasterize(&mut self, ch: char) -> Option<&GlyphEntry> {
            if !self.glyphs.contains_key(&ch) {
                self.rasterize(ch)?;
            }
            // Single lookup after the contains_key guard; entry API can't be
            // used here because rasterize() also borrows self mutably.
            self.glyphs.get(&ch)
        }

        fn rasterize(&mut self, ch: char) -> Option<()> {
            let cell_w = self.cell_w;
            let cell_h = self.cell_h;

            // Surrogate pairs not handled yet.
            if ch as u32 > 0xFFFF {
                return None;
            }

            // ── Glyph ID ─────────────────────────────────────────────────────────
            let chars: [u16; 1] = [ch as u16];
            let mut glyph: [CGGlyph; 1] = [0];
            unsafe {
                self.font.glyphs_for_characters(
                    NonNull::new(chars.as_ptr() as *mut _).unwrap(),
                    NonNull::new(glyph.as_mut_ptr()).unwrap(),
                    1,
                );
            }

            // ── CPU rasterization into a gray bitmap ─────────────────────────────
            // The Vec is zero-initialised → black background, no clearing needed.
            let mut pixels = vec![0u8; cell_w * cell_h];
            {
                let gray = CGColorSpace::new_device_gray()?;
                // kCGImageAlphaNone = 0: 1 byte per pixel, no alpha channel.
                // CGBitmapContextCreate uses our buffer directly; does not free it.
                let ctx = unsafe {
                    CGBitmapContextCreate(
                        pixels.as_mut_ptr() as *mut _,
                        cell_w,
                        cell_h,
                        8,      // bits per component
                        cell_w, // bytes per row (no padding)
                        Some(&gray),
                        0, // kCGImageAlphaNone | kCGBitmapByteOrderDefault
                    )
                }?;

                // White fill: [gray_component, alpha] for a gray colorspace.
                let white: [CGFloat; 2] = [1.0, 1.0];
                unsafe { CGContext::set_fill_color(Some(&ctx), white.as_ptr()) };

                // Draw glyph at the baseline.
                // CoreText Y-up: baseline is `descent` pixels from the bottom.
                let pos = CGPoint {
                    x: 0.0,
                    y: self.descent as f64,
                };
                unsafe {
                    self.font.draw_glyphs(
                        NonNull::new(glyph.as_mut_ptr()).unwrap(),
                        NonNull::new(&pos as *const _ as *mut _).unwrap(),
                        1,
                        &ctx,
                    );
                }
                // `ctx` drops here; pixels now hold the rendered bitmap.
            }

            // ── Pack into atlas ───────────────────────────────────────────────────
            let alloc = self
                .allocator
                .allocate(Size::new(cell_w as i32, cell_h as i32))?;
            let r = alloc.rectangle;
            let (ax, ay) = (r.min.x as usize, r.min.y as usize);

            // ── Upload to GPU ─────────────────────────────────────────────────────
            unsafe {
                self.texture
                    .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                        MTLRegion {
                            origin: MTLOrigin { x: ax, y: ay, z: 0 },
                            size: MTLSize {
                                width: cell_w,
                                height: cell_h,
                                depth: 1,
                            },
                        },
                        0,
                        NonNull::new(pixels.as_ptr() as *mut core::ffi::c_void).unwrap(),
                        cell_w,
                    );
            }

            // ── Record UVs ───────────────────────────────────────────────────────
            let s = ATLAS_SIZE as f32;
            self.glyphs.insert(
                ch,
                GlyphEntry {
                    uv_min: [ax as f32 / s, ay as f32 / s],
                    uv_max: [(ax + cell_w) as f32 / s, (ay + cell_h) as f32 / s],
                },
            );
            Some(())
        }
    }

    // ── Shared font cache for proportional text ────────────────────────────

    /// Caches sized CTFont instances and their line metrics.
    /// Shared between `ProportionalGlyphAtlas` (rasterization) and
    /// `PropTextMeasurer` (layout-only measurement).
    pub struct SizedFontCache {
        base_font: CFRetained<CTFont>,
        sized_fonts: HashMap<u16, CFRetained<CTFont>>,
        line_metrics: HashMap<u16, (f32, f32, f32)>,
        scale: f64,
    }

    impl SizedFontCache {
        pub fn new(base_font_size: f64, scale: f64) -> Option<Self> {
            let base_font = unsafe {
                CTFont::new_ui_font_for_language(CTFontUIFontType::System, base_font_size, None)
            }?;

            let size_tenths = (base_font_size * 10.0).round() as u16;
            let ascent = unsafe { base_font.ascent() } as f32;
            let descent = unsafe { base_font.descent() } as f32;
            let leading = unsafe { base_font.leading() } as f32;

            let mut sized_fonts = HashMap::new();
            sized_fonts.insert(size_tenths, base_font.clone());
            let mut line_metrics = HashMap::new();
            line_metrics.insert(size_tenths, (ascent, descent, leading));

            Some(Self {
                base_font,
                sized_fonts,
                line_metrics,
                scale,
            })
        }

        pub fn sized_font(&mut self, size_tenths: u16) -> &CTFont {
            if !self.sized_fonts.contains_key(&size_tenths) {
                let size = (size_tenths as f64 / 10.0) * self.scale;
                let font = unsafe {
                    self.base_font
                        .copy_with_attributes(size, std::ptr::null(), None)
                };
                let ascent = unsafe { font.ascent() } as f32;
                let descent = unsafe { font.descent() } as f32;
                let leading = unsafe { font.leading() } as f32;
                self.line_metrics
                    .insert(size_tenths, (ascent, descent, leading));
                self.sized_fonts.insert(size_tenths, font);
            }
            &self.sized_fonts[&size_tenths]
        }

        pub fn line_height(&mut self, size_tenths: u16) -> f32 {
            self.sized_font(size_tenths);
            let (a, d, l) = self.line_metrics[&size_tenths];
            (a + d + l).ceil()
        }

        pub fn ascent(&mut self, size_tenths: u16) -> f32 {
            self.sized_font(size_tenths);
            self.line_metrics[&size_tenths].0
        }

        pub fn descent(&mut self, size_tenths: u16) -> f32 {
            self.sized_font(size_tenths);
            self.line_metrics[&size_tenths].1
        }

        /// Measure the advance width of a single character at the given size.
        pub fn char_advance(&mut self, ch: char, size_tenths: u16) -> f32 {
            if ch as u32 > 0xFFFF {
                return 0.0;
            }
            let font = self.sized_font(size_tenths);
            let chars: [u16; 1] = [ch as u16];
            let mut glyph: [CGGlyph; 1] = [0];
            unsafe {
                font.glyphs_for_characters(
                    NonNull::new(chars.as_ptr() as *mut _).unwrap(),
                    NonNull::new(glyph.as_mut_ptr()).unwrap(),
                    1,
                );
            }
            let mut adv = CGSize {
                width: 0.0,
                height: 0.0,
            };
            unsafe {
                font.advances_for_glyphs(
                    CTFontOrientation::Default,
                    NonNull::new(glyph.as_mut_ptr()).unwrap(),
                    &mut adv,
                    1,
                );
            }
            adv.width as f32
        }

        /// Measure the total advance width of a string.
        pub fn measure_text(&mut self, text: &str, size_tenths: u16) -> f32 {
            let mut total = 0.0_f32;
            for ch in text.chars() {
                total += self.char_advance(ch, size_tenths);
            }
            total
        }
    }

    // ── Proportional glyph atlas ────────────────────────────────────────────

    const PROP_ATLAS_SIZE: usize = 2048;

    /// Per-character entry with individual glyph metrics for proportional fonts.
    #[derive(Clone, Copy, Debug)]
    pub struct ProportionalGlyphEntry {
        pub uv_min: [f32; 2],
        pub uv_max: [f32; 2],
        /// Horizontal advance in pixels.
        pub advance: f32,
        /// Rasterized bitmap width in pixels.
        pub raster_w: usize,
        /// Rasterized bitmap height in pixels.
        pub raster_h: usize,
    }

    pub struct ProportionalGlyphAtlas {
        pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
        allocator: AtlasAllocator,
        glyphs: HashMap<(char, u16), ProportionalGlyphEntry>,
        pub fonts: SizedFontCache,
    }

    impl ProportionalGlyphAtlas {
        pub fn new(
            device: &ProtocolObject<dyn MTLDevice>,
            base_font_size: f64,
            scale: f64,
        ) -> Option<Self> {
            let fonts = SizedFontCache::new(base_font_size, scale)?;

            let texture = {
                let desc = MTLTextureDescriptor::new();
                unsafe {
                    desc.setPixelFormat(MTLPixelFormat::R8Unorm);
                    desc.setWidth(PROP_ATLAS_SIZE);
                    desc.setHeight(PROP_ATLAS_SIZE);
                }
                device.newTextureWithDescriptor(&desc)?
            };

            Some(Self {
                texture,
                allocator: AtlasAllocator::new(Size::new(
                    PROP_ATLAS_SIZE as i32,
                    PROP_ATLAS_SIZE as i32,
                )),
                glyphs: HashMap::new(),
                fonts,
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
            let mut width = 0.0_f32;
            for ch in text.chars() {
                if let Some(entry) = self.get_or_rasterize(ch, size_tenths) {
                    width += entry.advance;
                }
            }
            width
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
            if ch as u32 > 0xFFFF {
                return None;
            }

            let line_h = self.fonts.line_height(size_tenths);
            let descent = self.fonts.descent(size_tenths);
            let advance = self.fonts.char_advance(ch, size_tenths);
            let font = self.fonts.sized_font(size_tenths);

            // ── Glyph ID ────────────────────────────────────────────────────
            let chars: [u16; 1] = [ch as u16];
            let mut glyph: [CGGlyph; 1] = [0];
            unsafe {
                font.glyphs_for_characters(
                    NonNull::new(chars.as_ptr() as *mut _).unwrap(),
                    NonNull::new(glyph.as_mut_ptr()).unwrap(),
                    1,
                );
            }

            if advance <= 0.0 {
                self.glyphs.insert(
                    (ch, size_tenths),
                    ProportionalGlyphEntry {
                        uv_min: [0.0, 0.0],
                        uv_max: [0.0, 0.0],
                        advance,
                        raster_w: 0,
                        raster_h: 0,
                    },
                );
                return Some(());
            }

            // Full line-height bitmap with shared baseline (like monospace).
            let raster_h = line_h as usize;
            let raster_w = (advance.ceil() as usize) + 4; // padding for smoothing

            let mut pixels = vec![0u8; raster_w * raster_h];
            {
                let gray = CGColorSpace::new_device_gray()?;
                let ctx = unsafe {
                    CGBitmapContextCreate(
                        pixels.as_mut_ptr() as *mut _,
                        raster_w,
                        raster_h,
                        8,
                        raster_w,
                        Some(&gray),
                        0,
                    )
                }?;

                // Enable font smoothing for crisp, professional text.
                CGContext::set_allows_font_smoothing(Some(&ctx), true);
                CGContext::set_should_smooth_fonts(Some(&ctx), true);
                CGContext::set_should_antialias(Some(&ctx), true);

                let white: [CGFloat; 2] = [1.0, 1.0];
                unsafe { CGContext::set_fill_color(Some(&ctx), white.as_ptr()) };

                // Draw at shared baseline (CoreText Y-up: y=descent from bottom).
                let pos = CGPoint {
                    x: 2.0, // center in padding
                    y: descent as f64,
                };
                unsafe {
                    font.draw_glyphs(
                        NonNull::new(glyph.as_mut_ptr()).unwrap(),
                        NonNull::new(&pos as *const _ as *mut _).unwrap(),
                        1,
                        &ctx,
                    );
                }
            }

            // ── Pack into atlas ─────────────────────────────────────────────
            let alloc = self
                .allocator
                .allocate(Size::new(raster_w as i32, raster_h as i32))?;
            let r = alloc.rectangle;
            let (ax, ay) = (r.min.x as usize, r.min.y as usize);

            unsafe {
                self.texture
                    .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                        MTLRegion {
                            origin: MTLOrigin { x: ax, y: ay, z: 0 },
                            size: MTLSize {
                                width: raster_w,
                                height: raster_h,
                                depth: 1,
                            },
                        },
                        0,
                        NonNull::new(pixels.as_ptr() as *mut core::ffi::c_void).unwrap(),
                        raster_w,
                    );
            }

            let s = PROP_ATLAS_SIZE as f32;
            self.glyphs.insert(
                (ch, size_tenths),
                ProportionalGlyphEntry {
                    uv_min: [ax as f32 / s, ay as f32 / s],
                    uv_max: [(ax + raster_w) as f32 / s, (ay + raster_h) as f32 / s],
                    advance,
                    raster_w,
                    raster_h,
                },
            );
            Some(())
        }
    }
}

#[cfg(target_os = "macos")]
pub use inner::*;
