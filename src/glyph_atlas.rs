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
    use objc2_core_text::{CTFont, CTFontOrientation};
    use objc2_metal::{
        MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLTexture,
        MTLTextureDescriptor,
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
                let mut adv = CGSize { width: 0.0, height: 0.0 };
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
                        0,      // kCGImageAlphaNone | kCGBitmapByteOrderDefault
                    )
                }?;

                // White fill: [gray_component, alpha] for a gray colorspace.
                let white: [CGFloat; 2] = [1.0, 1.0];
                unsafe { CGContext::set_fill_color(Some(&ctx), white.as_ptr()) };

                // Draw glyph at the baseline.
                // CoreText Y-up: baseline is `descent` pixels from the bottom.
                let pos = CGPoint { x: 0.0, y: self.descent as f64 };
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
                self.texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    MTLRegion {
                        origin: MTLOrigin { x: ax, y: ay, z: 0 },
                        size: MTLSize { width: cell_w, height: cell_h, depth: 1 },
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
}

#[cfg(target_os = "macos")]
pub use inner::*;
