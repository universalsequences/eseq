//! The agent's view of the user's window (`look`, `what_draws_live`): the
//! tiled frame the window itself builds, drawn offscreen and read. The
//! headless copy the other tools use has its own project; this is what the
//! user sees.

use eseqlisp::backend::{Backend, TileFrame, TiledRenderFrame};
use eseqlisp::ui::capture::PixelRegion;
use eseqlisp::Editor;
use sequencer::agent::harness::{LiveAction, ToolResult};

use crate::mcp::tools::{hook_lines, hooks_text};
use crate::AppBackend;

/// The longest side of an image `look` returns (the API's own limit): a
/// bigger tile is halved until it fits.
const MAX_IMAGE_SIDE: u32 = 1568;

/// Answer a `look` or `what_draws_live` call, or None for another action.
pub(super) fn serve(editor: &mut Editor, backend: &mut AppBackend, action: &LiveAction) -> Option<ToolResult> {
    if !matches!(action, LiveAction::Look { .. } | LiveAction::WhatDraws { .. }) {
        return None;
    }
    let (cols, rows) = backend.viewport_size();
    let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
    let result = match action {
        LiveAction::Look { buffer } => look(backend, &tiled, buffer.as_deref()),
        LiveAction::WhatDraws { buffer, x, y } => what_draws(backend, &tiled, buffer, (*x, *y)),
        _ => unreachable!(),
    };
    // Building the frame took the window's dirty widgets; hand them back so
    // its own next frame draws them.
    eseqlisp::frame::requeue_unpresented_tiled_frame(editor, &tiled);
    Some(result.unwrap_or_else(ToolResult::error))
}

/// How a tile appears in `look`'s image of it: its content's pixels in the
/// window, how many window pixels one image pixel spans, and the image's
/// cell size and size, in which its hooks are reported.
struct TileView<'a> {
    tile: &'a TileFrame,
    region: PixelRegion,
    scale: u32,
    cell: (f32, f32),
    size: (u32, u32),
}

/// The window's size in pixels: its drawable, or its whole cells without one.
fn window_size(backend: &AppBackend) -> (u32, u32) {
    backend.window_pixel_size().unwrap_or_else(|| {
        let (cols, rows) = backend.viewport_size();
        let cell = backend.cell_dimensions();
        ((cols as f32 * cell.0) as u32, (rows as f32 * cell.1) as u32)
    })
}

/// Halvings that bring `region` within [`MAX_IMAGE_SIDE`].
fn image_scale(region: PixelRegion) -> u32 {
    let mut scale = 1;
    while region.width.max(region.height) / scale > MAX_IMAGE_SIDE {
        scale *= 2;
    }
    scale
}

fn tile_view<'a>(backend: &AppBackend, tiled: &'a TiledRenderFrame, buffer: &str) -> Result<TileView<'a>, String> {
    let matching: Vec<&TileFrame> = tiled.tiles.iter().filter(|tile| tile.frame.buffer_name == buffer).collect();
    let tile = match matching.as_slice() {
        [tile] => *tile,
        [] => {
            let shown: Vec<&str> = tiled.tiles.iter().map(|tile| tile.frame.buffer_name.as_str()).collect();
            return Err(format!("No tile of the user's window shows {buffer}; they show: {}", shown.join(", ")));
        }
        _ => return Err(format!("{} tiles show {buffer}; `look` at the window to see them", matching.len())),
    };
    let cell = backend.cell_dimensions();
    // Widgets are laid out from the content's corner, inside the border.
    let inset = if tile.show_border {
        eseqlisp::widget_render::ui_design_px(tile.border_width_px).max(0.0)
    } else {
        0.0
    };
    let body = PixelRegion::from_rect(tile.body_rect, cell);
    let inset = inset.round() as u32;
    let (window_w, window_h) = window_size(backend);
    let x = (body.x + inset).min(window_w);
    let y = (body.y + inset).min(window_h);
    let region = PixelRegion {
        x,
        y,
        width: body.width.saturating_sub(2 * inset).min(window_w - x),
        height: body.height.saturating_sub(2 * inset).min(window_h - y),
    };
    let scale = image_scale(region);
    let factor = scale as f32;
    Ok(TileView {
        tile,
        region,
        scale,
        cell: (cell.0 / factor, cell.1 / factor),
        size: (region.width / scale, region.height / scale),
    })
}

fn look(backend: &mut AppBackend, tiled: &TiledRenderFrame, buffer: Option<&str>) -> Result<ToolResult, String> {
    let (width, height) = window_size(backend);
    let window = PixelRegion { x: 0, y: 0, width, height };
    let (region, scale, text) = match buffer {
        None => {
            let scale = image_scale(window);
            let cell = backend.cell_dimensions();
            let mut lines = vec!["The user's window; its tiles show (look at one by its buffer for its hooks):".to_string()];
            lines.extend(tiled.tiles.iter().map(|tile| {
                let place = PixelRegion::from_rect(tile.body_rect, cell);
                let place = PixelRegion { x: place.x / scale, y: place.y / scale,
                    width: place.width / scale, height: place.height / scale };
                format!("  {} at {place}", tile.frame.buffer_name)
            }));
            (window, scale, lines.join("\n"))
        }
        Some(buffer) => {
            let view = tile_view(backend, tiled, buffer)?;
            let mut lines = vec![format!(
                "{buffer} in the user's window, {}x{} px (what_draws_live takes pixels of this image).",
                view.size.0, view.size.1
            )];
            let report = eseqlisp::ui::capture::layout_report(&view.tile.frame, view.cell, view.size)?;
            lines.extend(hook_lines(&report.hooks));
            (view.region, view.scale, lines.join("\n"))
        }
    };
    let target = backend
        .create_tiled_capture_target(width, height)
        .map_err(|_| "could not make an image of the window".to_string())?;
    backend
        .render_tiled_capture(tiled, &target)
        .map_err(|_| "could not draw the window offscreen".to_string())?;
    Ok(ToolResult::ok(text).with_png(target.region_png(region, scale)?))
}

fn what_draws(
    backend: &AppBackend,
    tiled: &TiledRenderFrame,
    buffer: &str,
    point: (f32, f32),
) -> Result<ToolResult, String> {
    let view = tile_view(backend, tiled, buffer)?;
    let stack = eseqlisp::ui::capture::hooks_at(&view.tile.frame, view.cell, view.size, point)?;
    Ok(ToolResult::ok(hooks_text(buffer, point, &stack)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_shows_the_window_and_names_the_hooks_of_a_tile() {
        let mut editor = Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        let mut backend = crate::editor_setup::create_offscreen_capture_backend(&mut editor, 640, 384, 1.0)
            .expect("offscreen backend");
        editor
            .runtime_mut()
            .eval_str(r#"(module test.live)
                (def plate () (box :key "plate" :part "test/plate" :width 10 :height 4
                  :background-color :accent))
                (effect (v-stack (plate)))"#)
            .unwrap();
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        let (cols, rows) = backend.viewport_size();
        editor.update_tile_rects(cols as u16, rows as u16);
        let buffer = editor.active_buffer().name.clone();

        let window = serve(&mut editor, &mut backend, &LiveAction::Look { buffer: None }).unwrap();
        assert!(!window.is_error, "{}", window.text());
        assert_eq!(window.images.len(), 1, "an image of the window");
        assert!(window.text().contains(&buffer), "{}", window.text());

        let tile = serve(&mut editor, &mut backend, &LiveAction::Look { buffer: Some(buffer.clone()) }).unwrap();
        assert!(!tile.is_error, "{}", tile.text());
        assert_eq!(tile.content[0]["type"], "image");
        assert!(tile.text().contains("test/plate"), "{}", tile.text());
        assert!(tile.text().contains("test.live/plate"), "{}", tile.text());

        let at = serve(&mut editor, &mut backend, &LiveAction::WhatDraws { buffer: buffer.clone(), x: 20.0, y: 20.0 })
            .unwrap();
        assert!(at.text().contains("restyle \"test/plate\""), "{}", at.text());

        let missing = serve(&mut editor, &mut backend, &LiveAction::Look { buffer: Some("*nope*".into()) }).unwrap();
        assert!(missing.is_error && missing.text().contains(&buffer), "{}", missing.text());
    }
}
