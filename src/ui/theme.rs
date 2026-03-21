//! Dark industrial theme — centralized palette for both TUI and Metal backends.
//!
//! All editor colors live here so a theme change is a single-file edit.
//! Inspired by dark sequencer/synth UIs with chartreuse accent.

use crate::backend::Color;

// ── Base palette ─────────────────────────────────────────────────────────────

pub const BG: Color = Color::from_hex(0x0a, 0x0a, 0x0a);
pub const FG: Color = Color::from_hex(0xe0, 0xe0, 0xe0);
pub const FG_MUTED: Color = Color::from_hex(0x50, 0x50, 0x50);

pub const BLACK: Color = Color::from_hex(0x05, 0x05, 0x05);
pub const RED: Color = Color::from_hex(0xff, 0x3b, 0x3b);
pub const GREEN: Color = Color::from_hex(0xc8, 0xff, 0x00);
pub const YELLOW: Color = Color::from_hex(0xc8, 0xff, 0x00);
pub const BLUE: Color = Color::from_hex(0x5a, 0x9e, 0xff);
pub const MAGENTA: Color = Color::from_hex(0xc8, 0xff, 0x00);
pub const CYAN: Color = Color::from_hex(0x61, 0xff, 0xca);
pub const WHITE: Color = Color::from_hex(0xe0, 0xe0, 0xe0);

pub const BRIGHT_BLACK: Color = Color::from_hex(0x3a, 0x3a, 0x3a);
pub const BRIGHT_RED: Color = Color::from_hex(0xff, 0x6b, 0x6b);
pub const BRIGHT_YELLOW: Color = Color::from_hex(0xd4, 0xff, 0x40);

pub const PURPLE: Color = Color::from_hex(0xc8, 0xff, 0x00);
pub const CURSOR: Color = PURPLE;

// ── Syntax highlighting ──────────────────────────────────────────────────────

pub const SYN_COMMENT: Color = BRIGHT_BLACK;
pub const SYN_STRING: Color = CYAN;
pub const SYN_NUMBER: Color = Color::from_hex(0xff, 0xca, 0x85);
pub const SYN_KEYWORD: Color = GREEN;
pub const SYN_BUILTIN: Color = Color::from_hex(0xff, 0xca, 0x85);
pub const SYN_SPECIAL: Color = BLUE;
pub const SYN_DELIMITER: Color = Color::from_hex(0x55, 0x55, 0x55);

// ── Semantic regions ─────────────────────────────────────────────────────────

/// Selection / active region background
pub const BG_REGION: Color = Color::rgba(0.784, 1.0, 0.0, 0.20);
/// Enclosing s-expression background
pub const BG_SEXP: Color = Color::rgba(0.12, 0.14, 0.05, 1.0);
/// Eval flash (brief highlight after evaluating)
pub const BG_EVAL_FLASH: Color = Color::rgba(0.784, 1.0, 0.0, 0.20);
/// Matching parenthesis background
pub const BG_MATCH_PAREN: Color = GREEN;
/// Matching parenthesis foreground
pub const FG_MATCH_PAREN: Color = BLACK;

// ── UI chrome ────────────────────────────────────────────────────────────────

pub const STATUS_FG: Color = FG;
pub const STATUS_BG: Color = Color::from_hex(0x14, 0x14, 0x14);

// ── Completion popup ─────────────────────────────────────────────────────────

pub const COMP_SELECTED_BG: Color = Color::from_hex(0x2a, 0x2e, 0x10);
pub const COMP_UNSELECTED_BG: Color = Color::from_hex(0x14, 0x14, 0x14);
pub const COMP_FG: Color = FG;
pub const COMP_DOC_BG: Color = Color::from_hex(0x0d, 0x0d, 0x0d);
pub const COMP_DOC_FG: Color = FG;
pub const COMP_DOC_TITLE_FG: Color = GREEN;

// ── Widgets ──────────────────────────────────────────────────────────────────

pub const WIDGET_LABEL_FG: Color = FG;
pub const WIDGET_SLIDER_FILLED: Color = GREEN;
pub const WIDGET_SLIDER_TRACK: Color = BRIGHT_BLACK;
pub const WIDGET_KNOB_FILLED: Color = GREEN;
pub const WIDGET_KNOB_TRACK: Color = BRIGHT_BLACK;
pub const WIDGET_TOGGLE_ON: Color = GREEN;
pub const WIDGET_TOGGLE_OFF: Color = BRIGHT_BLACK;
