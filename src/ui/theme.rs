//! Aura color theme — centralized palette for both TUI and Metal backends.
//!
//! All editor colors live here so a theme change is a single-file edit.
//! Values sourced from the Ghostty "Aura" theme.

use crate::backend::Color;

// ── Base palette ─────────────────────────────────────────────────────────────

pub const BG: Color = Color::from_hex(0x15, 0x14, 0x1b);
pub const FG: Color = Color::from_hex(0xed, 0xec, 0xee);
pub const FG_MUTED: Color = Color::from_hex(0x4d, 0x4c, 0x4e);

pub const BLACK: Color = Color::from_hex(0x11, 0x0f, 0x18);
pub const RED: Color = Color::from_hex(0xff, 0x67, 0x67);
pub const GREEN: Color = Color::from_hex(0x61, 0xff, 0xca);
pub const YELLOW: Color = Color::from_hex(0xff, 0xca, 0x85);
pub const BLUE: Color = Color::from_hex(0xa2, 0x77, 0xff);
pub const MAGENTA: Color = Color::from_hex(0xa2, 0x77, 0xff);
pub const CYAN: Color = Color::from_hex(0x61, 0xff, 0xca);
pub const WHITE: Color = Color::from_hex(0xed, 0xec, 0xee);

pub const BRIGHT_BLACK: Color = Color::from_hex(0x4d, 0x4d, 0x4d);
pub const BRIGHT_RED: Color = Color::from_hex(0xff, 0xca, 0x85);
pub const BRIGHT_YELLOW: Color = Color::from_hex(0xff, 0xca, 0x85);

pub const PURPLE: Color = Color::from_hex(0xa2, 0x77, 0xff);
pub const CURSOR: Color = PURPLE;

// ── Syntax highlighting ──────────────────────────────────────────────────────

pub const SYN_COMMENT: Color = BRIGHT_BLACK;
pub const SYN_STRING: Color = GREEN;
pub const SYN_NUMBER: Color = CYAN;
pub const SYN_KEYWORD: Color = MAGENTA;
pub const SYN_BUILTIN: Color = YELLOW;
pub const SYN_SPECIAL: Color = BLUE;
pub const SYN_DELIMITER: Color = Color::from_hex(0x6d, 0x6d, 0x6d);

// ── Semantic regions ─────────────────────────────────────────────────────────

/// Selection / active region background
pub const BG_REGION: Color = Color::rgba(0.635, 0.467, 1.0, 0.30);
/// Enclosing s-expression background
pub const BG_SEXP: Color = Color::rgba(0.180, 0.145, 0.280, 1.0);
/// Eval flash (brief highlight after evaluating)
pub const BG_EVAL_FLASH: Color = Color::rgba(0.635, 0.467, 1.0, 0.25);
/// Matching parenthesis background
pub const BG_MATCH_PAREN: Color = PURPLE;
/// Matching parenthesis foreground
pub const FG_MATCH_PAREN: Color = Color::from_hex(0x15, 0x14, 0x1b);

// ── UI chrome ────────────────────────────────────────────────────────────────

pub const STATUS_FG: Color = FG;
pub const STATUS_BG: Color = Color::from_hex(0x1e, 0x1c, 0x28);

// ── Completion popup ─────────────────────────────────────────────────────────

pub const COMP_SELECTED_BG: Color = Color::from_hex(0x54, 0x4e, 0x96);
pub const COMP_UNSELECTED_BG: Color = Color::from_hex(0x1e, 0x1c, 0x28);
pub const COMP_FG: Color = FG;
pub const COMP_DOC_BG: Color = Color::from_hex(0x11, 0x0f, 0x18);
pub const COMP_DOC_FG: Color = FG;
pub const COMP_DOC_TITLE_FG: Color = PURPLE;

// ── Widgets ──────────────────────────────────────────────────────────────────

pub const WIDGET_LABEL_FG: Color = FG;
pub const WIDGET_SLIDER_FILLED: Color = PURPLE;
pub const WIDGET_SLIDER_TRACK: Color = BRIGHT_BLACK;
pub const WIDGET_KNOB_FILLED: Color = PURPLE;
pub const WIDGET_KNOB_TRACK: Color = BRIGHT_BLACK;
pub const WIDGET_TOGGLE_ON: Color = PURPLE;
pub const WIDGET_TOGGLE_OFF: Color = BRIGHT_BLACK;
