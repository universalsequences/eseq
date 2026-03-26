//! Dynamic theme palette shared by the TUI and Metal backends.

use std::sync::{OnceLock, RwLock};

use crate::backend::Color;
use crate::vm::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub accent: Color,
    pub bg: Color,
    pub fg: Color,
    pub fg_muted: Color,
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
    pub bright_black: Color,
    pub bright_red: Color,
    pub bright_yellow: Color,
    pub purple: Color,
    pub cursor: Color,
    pub syn_comment: Color,
    pub syn_string: Color,
    pub syn_number: Color,
    pub syn_keyword: Color,
    pub syn_builtin: Color,
    pub syn_special: Color,
    pub syn_delimiter: Color,
    pub bg_region: Color,
    pub bg_sexp: Color,
    pub bg_eval_flash: Color,
    pub bg_match_paren: Color,
    pub fg_match_paren: Color,
    pub status_fg: Color,
    pub status_bg: Color,
    pub status_edge: Color,
    pub status_chip_bg: Color,
    pub status_mode_bg: Color,
    pub status_chip_muted: Color,
    pub status_ui_bg: Color,
    pub status_ui_fg: Color,
    pub status_mix_bg: Color,
    pub status_mix_fg: Color,
    pub status_dirty_bg: Color,
    pub status_dirty_fg: Color,
    pub status_pos_bg: Color,
    pub status_accent: Color,
    pub comp_selected_bg: Color,
    pub comp_unselected_bg: Color,
    pub comp_fg: Color,
    pub comp_doc_bg: Color,
    pub comp_doc_fg: Color,
    pub comp_doc_title_fg: Color,
    pub widget_focus_bg: Color,
    pub widget_label_fg: Color,
    pub widget_slider_filled: Color,
    pub widget_slider_track: Color,
    pub widget_knob_filled: Color,
    pub widget_knob_track: Color,
    pub widget_toggle_on: Color,
    pub widget_toggle_off: Color,
    pub widget_toggle_knob_on: Color,
    pub widget_toggle_knob_off: Color,
    pub border_active: Color,
    pub border_inactive: Color,
}

macro_rules! theme_slots {
    ($(($field:ident, $getter:ident, $default:expr)),+ $(,)?) => {
        pub fn default_theme() -> Theme {
            Theme {
                $($field: $default,)+
            }
        }

        pub fn reactive_fields() -> Vec<(&'static str, Value)> {
            let theme = default_theme();
            vec![
                $((stringify!($field), color_to_value(theme.$field)),)+
            ]
        }

        pub fn sync_from_value(value: &Value) {
            let mut theme = current();
            let prev = theme;
            if let Value::Map(map) = value {
                $(
                    if let Some(value) = map.get(stringify!($field)) {
                        if let Some(color) = parse_color_value(&value.borrow()) {
                            theme.$field = color;
                        }
                    }
                )+
            }
            if theme != prev {
                set_current(theme);
            }
        }

        pub fn named_color(name: &str) -> Option<Color> {
            let normalized = normalize_name(name);
            let theme = current();
            match normalized.as_str() {
                "primary" => Some(theme.accent),
                "secondary" => Some(theme.red),
                "gray" | "grey" | "dim" => Some(theme.bright_black),
                $(
                    stringify!($field) => Some(theme.$field),
                )+
                _ => None,
            }
        }

        $(
            #[allow(non_snake_case)]
            pub fn $getter() -> Color {
                current().$field
            }
        )+
    };
}

theme_slots!(
    (accent, ACCENT, Color::from_hex(0xc8, 0xff, 0x00)),
    (bg, BG, Color::from_hex(0x0a, 0x0a, 0x0a)),
    (fg, FG, Color::from_hex(0xe0, 0xe0, 0xe0)),
    (fg_muted, FG_MUTED, Color::from_hex(0x50, 0x50, 0x50)),
    (black, BLACK, Color::from_hex(0x05, 0x05, 0x05)),
    (red, RED, Color::from_hex(0xff, 0x3b, 0x3b)),
    (green, GREEN, Color::from_hex(0xc8, 0xff, 0x00)),
    (yellow, YELLOW, Color::from_hex(0xc8, 0xff, 0x00)),
    (blue, BLUE, Color::from_hex(0x5a, 0x9e, 0xff)),
    (magenta, MAGENTA, Color::from_hex(0xc8, 0xff, 0x00)),
    (cyan, CYAN, Color::from_hex(0x61, 0xff, 0xca)),
    (white, WHITE, Color::from_hex(0xe0, 0xe0, 0xe0)),
    (
        bright_black,
        BRIGHT_BLACK,
        Color::from_hex(0x3a, 0x3a, 0x3a)
    ),
    (bright_red, BRIGHT_RED, Color::from_hex(0xff, 0x6b, 0x6b)),
    (
        bright_yellow,
        BRIGHT_YELLOW,
        Color::from_hex(0xd4, 0xff, 0x40)
    ),
    (purple, PURPLE, Color::from_hex(0xc8, 0xff, 0x00)),
    (cursor, CURSOR, Color::from_hex(0xc8, 0xff, 0x00)),
    (syn_comment, SYN_COMMENT, Color::from_hex(0x3a, 0x3a, 0x3a)),
    (syn_string, SYN_STRING, Color::from_hex(0x61, 0xff, 0xca)),
    (syn_number, SYN_NUMBER, Color::from_hex(0xff, 0xca, 0x85)),
    (syn_keyword, SYN_KEYWORD, Color::from_hex(0xc8, 0xff, 0x00)),
    (syn_builtin, SYN_BUILTIN, Color::from_hex(0xff, 0xca, 0x85)),
    (syn_special, SYN_SPECIAL, Color::from_hex(0x5a, 0x9e, 0xff)),
    (
        syn_delimiter,
        SYN_DELIMITER,
        Color::from_hex(0x55, 0x55, 0x55)
    ),
    (bg_region, BG_REGION, Color::from_hex(0xb6, 0xd8, 0x2d)),
    (bg_sexp, BG_SEXP, Color::rgba(0.12, 0.14, 0.05, 1.0)),
    (
        bg_eval_flash,
        BG_EVAL_FLASH,
        Color::rgba(0.784, 1.0, 0.0, 0.20)
    ),
    (
        bg_match_paren,
        BG_MATCH_PAREN,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        fg_match_paren,
        FG_MATCH_PAREN,
        Color::from_hex(0x05, 0x05, 0x05)
    ),
    (status_fg, STATUS_FG, Color::from_hex(0xe0, 0xe0, 0xe0)),
    (status_bg, STATUS_BG, Color::from_hex(0x14, 0x14, 0x14)),
    (status_edge, STATUS_EDGE, Color::from_hex(0x23, 0x23, 0x23)),
    (
        status_chip_bg,
        STATUS_CHIP_BG,
        Color::from_hex(0x1f, 0x1f, 0x1f)
    ),
    (
        status_mode_bg,
        STATUS_MODE_BG,
        Color::from_hex(0x2c, 0x2c, 0x2c)
    ),
    (
        status_chip_muted,
        STATUS_CHIP_MUTED,
        Color::from_hex(0x18, 0x23, 0x12)
    ),
    (
        status_ui_bg,
        STATUS_UI_BG,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        status_ui_fg,
        STATUS_UI_FG,
        Color::from_hex(0x05, 0x05, 0x05)
    ),
    (
        status_mix_bg,
        STATUS_MIX_BG,
        Color::from_hex(0x5a, 0x9e, 0xff)
    ),
    (
        status_mix_fg,
        STATUS_MIX_FG,
        Color::from_hex(0x05, 0x05, 0x05)
    ),
    (
        status_dirty_bg,
        STATUS_DIRTY_BG,
        Color::from_hex(0x4a, 0x33, 0x10)
    ),
    (
        status_dirty_fg,
        STATUS_DIRTY_FG,
        Color::from_hex(0xff, 0xde, 0xa6)
    ),
    (
        status_pos_bg,
        STATUS_POS_BG,
        Color::from_hex(0x10, 0x10, 0x10)
    ),
    (
        status_accent,
        STATUS_ACCENT,
        Color::from_hex(0x61, 0xff, 0xca)
    ),
    (
        comp_selected_bg,
        COMP_SELECTED_BG,
        Color::from_hex(0x2a, 0x2e, 0x10)
    ),
    (
        comp_unselected_bg,
        COMP_UNSELECTED_BG,
        Color::from_hex(0x14, 0x14, 0x14)
    ),
    (comp_fg, COMP_FG, Color::from_hex(0xe0, 0xe0, 0xe0)),
    (comp_doc_bg, COMP_DOC_BG, Color::from_hex(0x0d, 0x0d, 0x0d)),
    (comp_doc_fg, COMP_DOC_FG, Color::from_hex(0xe0, 0xe0, 0xe0)),
    (
        comp_doc_title_fg,
        COMP_DOC_TITLE_FG,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        widget_focus_bg,
        WIDGET_FOCUS_BG,
        Color::from_hex(0x2f, 0x36, 0x12)
    ),
    (
        widget_label_fg,
        WIDGET_LABEL_FG,
        Color::from_hex(0xe0, 0xe0, 0xe0)
    ),
    (
        widget_slider_filled,
        WIDGET_SLIDER_FILLED,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        widget_slider_track,
        WIDGET_SLIDER_TRACK,
        Color::from_hex(0x3a, 0x3a, 0x3a)
    ),
    (
        widget_knob_filled,
        WIDGET_KNOB_FILLED,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        widget_knob_track,
        WIDGET_KNOB_TRACK,
        Color::from_hex(0x3a, 0x3a, 0x3a)
    ),
    (
        widget_toggle_on,
        WIDGET_TOGGLE_ON,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        widget_toggle_off,
        WIDGET_TOGGLE_OFF,
        Color::from_hex(0x6f, 0x7a, 0x8f)
    ),
    (
        widget_toggle_knob_on,
        WIDGET_TOGGLE_KNOB_ON,
        Color::from_hex(0xff, 0xff, 0xff)
    ),
    (
        widget_toggle_knob_off,
        WIDGET_TOGGLE_KNOB_OFF,
        Color::from_hex(0xf1, 0xf3, 0xf7)
    ),
    (
        border_active,
        BORDER_ACTIVE,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        border_inactive,
        BORDER_INACTIVE,
        Color::from_hex(0x3a, 0x3a, 0x3a)
    ),
);

static ACTIVE_THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

fn active_theme() -> &'static RwLock<Theme> {
    ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme()))
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonically increasing generation counter — bumped on every `set_current`.
static THEME_GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Per-thread snapshot of the theme, refreshed when the generation changes.
    static CACHED_THEME: std::cell::Cell<(u64, Theme)> = std::cell::Cell::new((0, default_theme()));
}

/// Read the theme, using a thread-local cache to avoid lock contention.
/// The cache is invalidated whenever `set_current` is called.
pub fn current() -> Theme {
    let current_gen = THEME_GENERATION.load(Ordering::Relaxed);
    CACHED_THEME.with(|cell| {
        let (cached_gen, cached_theme) = cell.get();
        if cached_gen == current_gen {
            return cached_theme;
        }
        let theme = *active_theme()
            .read()
            .expect("theme lock should not be poisoned");
        cell.set((current_gen, theme));
        theme
    })
}

pub fn set_current(theme: Theme) {
    *active_theme()
        .write()
        .expect("theme lock should not be poisoned") = theme;
    THEME_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn parse_color_value(value: &Value) -> Option<Color> {
    match value {
        Value::String(text) | Value::Keyword(text) => parse_color_string(text),
        Value::List(items) => parse_color_list(items)
            .or_else(|| parse_color_func_call(items)),
        _ => None,
    }
}

/// Handle (rgba r g b a) and (rgb r g b) function-call forms as colors.
fn parse_color_func_call(items: &[std::rc::Rc<std::cell::RefCell<Value>>]) -> Option<Color> {
    let head = items.first()?;
    let name = match &*head.borrow() {
        Value::Symbol(s) => s.clone(),
        _ => return None,
    };
    let nums: Vec<f32> = items[1..]
        .iter()
        .map(|item| match &*item.borrow() {
            Value::Number(n) => Some(*n as f32),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    match (name.as_str(), nums.as_slice()) {
        ("rgba", [r, g, b, a]) => Some(Color::rgba(*r, *g, *b, *a)),
        ("rgb", [r, g, b]) => Some(Color::rgb(*r, *g, *b)),
        _ => None,
    }
}

fn parse_color_string(text: &str) -> Option<Color> {
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    named_color(text)
}

fn parse_color_list(items: &[std::rc::Rc<std::cell::RefCell<Value>>]) -> Option<Color> {
    let components = items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::Number(n) => Some(*n as f32),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    match components.as_slice() {
        [r, g, b] => Some(Color::rgb(*r, *g, *b)),
        [r, g, b, a] => Some(Color::rgba(*r, *g, *b, *a)),
        _ => None,
    }
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(Color::rgba(
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ))
        }
        _ => None,
    }
}

fn color_to_value(color: Color) -> Value {
    Value::String(format!(
        "#{:02x}{:02x}{:02x}{}",
        to_u8(color.r),
        to_u8(color.g),
        to_u8(color.b),
        alpha_suffix(color.a)
    ))
}

fn alpha_suffix(alpha: f32) -> String {
    if (alpha - 1.0).abs() < f32::EPSILON {
        String::new()
    } else {
        format!("{:02x}", to_u8(alpha))
    }
}

fn to_u8(component: f32) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn normalize_name(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .replace(' ', "_")
}
