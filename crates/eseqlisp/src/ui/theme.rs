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
    pub dim: Color,
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
    pub buffer_bg: Color,
    pub buffer_tab_bar_bg: Color,
    pub buffer_tab_selected_bg: Color,
    pub buffer_tab_selected_border: Color,
    pub buffer_tab_fg: Color,
    pub buffer_tab_selected_fg: Color,
    pub buffer_tab_selected_highlight: Color,
    pub buffer_tab_selected_shadow: Color,
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
    pub tree_row_alt_bg: Color,
    pub fx_panel_bg: Color,
    pub fx_inner_panel_bg: Color,
    pub fx_panel_selected_bg: Color,
    pub fx_panel_header_bg: Color,
    pub fx_panel_header_selected_bg: Color,
    pub fx_panel_border: Color,
    pub instrument_panel_bg: Color,
    pub instrument_control_bg: Color,
    pub instrument_group_bg: Color,
    pub instrument_group_selected_bg: Color,
    pub mixer_strip_bg: Color,
    pub mixer_strip_selected_bg: Color,
    pub mixer_strip_muted_bg: Color,
    pub mixer_strip_border: Color,
    pub mixer_strip_selected_border: Color,
    pub mixer_control_bg: Color,
    pub mixer_label_bg: Color,
    pub mixer_label_muted_bg: Color,
    pub button_primary_bg: Color,
    pub button_primary_fg: Color,
    pub button_secondary_bg: Color,
    pub button_secondary_fg: Color,
    pub button_ghost_bg: Color,
    pub button_ghost_fg: Color,
    pub button_danger_bg: Color,
    pub button_danger_fg: Color,
    pub button_border: Color,
    pub button_highlight: Color,
    pub button_shadow: Color,
    pub dropdown_bg: Color,
    pub dropdown_fg: Color,
    pub dropdown_ring: Color,
    pub dropdown_chevron: Color,
    pub dropdown_badge_bg: Color,
    pub dropdown_menu_bg: Color,
    pub dropdown_menu_border: Color,
    pub dropdown_hover_bg: Color,
    pub dropdown_check: Color,
    pub dropdown_scrollbar: Color,
    pub inspect_overlay_fill: Color,
    pub inspect_overlay_border: Color,
    pub widget_focus_bg: Color,
    pub widget_label_fg: Color,
    pub widget_slider_filled: Color,
    pub widget_slider_track: Color,
    pub widget_slider_dot: Color,
    pub widget_knob_filled: Color,
    pub widget_knob_track: Color,
    pub widget_toggle_on: Color,
    pub widget_toggle_off: Color,
    pub widget_toggle_knob_on: Color,
    pub widget_toggle_knob_off: Color,
    pub patcher_bg: Color,
    pub patcher_grid_minor: Color,
    pub patcher_grid_major: Color,
    pub patcher_text: Color,
    pub patcher_text_muted: Color,
    pub patcher_error: Color,
    pub patcher_cable: Color,
    pub patcher_feedback_cable: Color,
    pub patcher_marquee_fill: Color,
    pub patcher_marquee_border: Color,
    pub patcher_node_bg: Color,
    pub patcher_node_border: Color,
    pub patcher_node_text: Color,
    pub patcher_node_tail_text: Color,
    pub patcher_io_node_bg: Color,
    pub patcher_io_node_border: Color,
    pub patcher_io_node_text: Color,
    pub patcher_param_node_bg: Color,
    pub patcher_param_node_border: Color,
    pub patcher_param_node_text: Color,
    pub patcher_code_node_bg: Color,
    pub patcher_code_node_border: Color,
    pub patcher_code_node_text: Color,
    pub patcher_node_hover_border: Color,
    pub patcher_node_selected_border: Color,
    pub patcher_port_input: Color,
    pub patcher_port_output: Color,
    pub patcher_edit_selection: Color,
    pub patcher_edit_cursor: Color,
    pub patcher_alignment_guide: Color,
    pub patcher_autocomplete_bg: Color,
    pub patcher_autocomplete_border: Color,
    pub patcher_autocomplete_selected_bg: Color,
    pub patcher_back_button_bg: Color,
    pub patcher_back_button_hover_bg: Color,
    pub patcher_back_button_border: Color,
    pub patcher_back_button_hover_border: Color,
    pub patcher_back_button_text: Color,
    pub patcher_back_button_hover_text: Color,
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
            let theme = current();
            vec![
                $((stringify!($field), color_to_value(theme.$field)),)+
            ]
        }

        pub fn sync_from_value(value: &Value) {
            let mut theme = current();
            let prev = theme;
            if let Value::Map(map) = value {
                $(
                    if let Some(value) = map.get(stringify!($field)).or_else(|| {
                        map.iter()
                            .find(|(key, _)| normalize_name(key) == stringify!($field))
                            .map(|(_, value)| value)
                    }) {
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
            let theme = current();
            named_color_in(&theme, name)
        }

        pub fn named_color_in(theme: &Theme, name: &str) -> Option<Color> {
            let normalized = normalize_name(name);
            match normalized.as_str() {
                "primary" => Some(theme.accent),
                "secondary" => Some(theme.red),
                "gray" | "grey" => Some(theme.bright_black),
                "dim" => Some(theme.dim),
                "transparent" | "none" => Some(Color::rgba(0.0, 0.0, 0.0, 0.0)),
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
    (dim, DIM, Color::from_hex(0x6c, 0x6c, 0x70)),
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
    (buffer_bg, BUFFER_BG, Color::from_hex(0x12, 0x12, 0x13)),
    (
        buffer_tab_bar_bg,
        BUFFER_TAB_BAR_BG,
        Color::from_hex(0x14, 0x14, 0x14)
    ),
    (
        buffer_tab_selected_bg,
        BUFFER_TAB_SELECTED_BG,
        Color::from_hex(0x48, 0x48, 0x4d)
    ),
    (
        buffer_tab_selected_border,
        BUFFER_TAB_SELECTED_BORDER,
        Color::from_hex(0x68, 0x68, 0x70)
    ),
    (
        buffer_tab_fg,
        BUFFER_TAB_FG,
        Color::from_hex(0x8a, 0x8d, 0x92)
    ),
    (
        buffer_tab_selected_fg,
        BUFFER_TAB_SELECTED_FG,
        Color::from_hex(0xe8, 0xe8, 0xea)
    ),
    (
        buffer_tab_selected_highlight,
        BUFFER_TAB_SELECTED_HIGHLIGHT,
        Color::rgba(1.0, 1.0, 1.0, 0.18)
    ),
    (
        buffer_tab_selected_shadow,
        BUFFER_TAB_SELECTED_SHADOW,
        Color::rgba(0.0, 0.0, 0.0, 0.22)
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
        tree_row_alt_bg,
        TREE_ROW_ALT_BG,
        Color::from_hex(0x18, 0x18, 0x19)
    ),
    (fx_panel_bg, FX_PANEL_BG, Color::from_hex(0x22, 0x22, 0x23)),
    (
        fx_inner_panel_bg,
        FX_INNER_PANEL_BG,
        Color::from_hex(0x1b, 0x1b, 0x1c)
    ),
    (
        fx_panel_selected_bg,
        FX_PANEL_SELECTED_BG,
        Color::from_hex(0x2a, 0x2a, 0x2d)
    ),
    (
        fx_panel_header_bg,
        FX_PANEL_HEADER_BG,
        Color::from_hex(0x20, 0x20, 0x21)
    ),
    (
        fx_panel_header_selected_bg,
        FX_PANEL_HEADER_SELECTED_BG,
        Color::from_hex(0x2a, 0x2a, 0x2d)
    ),
    (
        fx_panel_border,
        FX_PANEL_BORDER,
        Color::from_hex(0x30, 0x30, 0x33)
    ),
    (
        instrument_panel_bg,
        INSTRUMENT_PANEL_BG,
        Color::from_hex(0x22, 0x22, 0x23)
    ),
    (
        instrument_control_bg,
        INSTRUMENT_CONTROL_BG,
        Color::from_hex(0x17, 0x17, 0x18)
    ),
    (
        instrument_group_bg,
        INSTRUMENT_GROUP_BG,
        Color::from_hex(0x12, 0x12, 0x13)
    ),
    (
        instrument_group_selected_bg,
        INSTRUMENT_GROUP_SELECTED_BG,
        Color::from_hex(0x20, 0x20, 0x22)
    ),
    (
        mixer_strip_bg,
        MIXER_STRIP_BG,
        Color::from_hex(0x22, 0x22, 0x24)
    ),
    (
        mixer_strip_selected_bg,
        MIXER_STRIP_SELECTED_BG,
        Color::from_hex(0x28, 0x28, 0x2b)
    ),
    (
        mixer_strip_muted_bg,
        MIXER_STRIP_MUTED_BG,
        Color::from_hex(0x18, 0x18, 0x1a)
    ),
    (
        mixer_strip_border,
        MIXER_STRIP_BORDER,
        Color::from_hex(0x38, 0x38, 0x3a)
    ),
    (
        mixer_strip_selected_border,
        MIXER_STRIP_SELECTED_BORDER,
        Color::from_hex(0x6b, 0x6b, 0x70)
    ),
    (
        mixer_control_bg,
        MIXER_CONTROL_BG,
        Color::from_hex(0x12, 0x13, 0x14)
    ),
    (
        mixer_label_bg,
        MIXER_LABEL_BG,
        Color::from_hex(0x21, 0x21, 0x24)
    ),
    (
        mixer_label_muted_bg,
        MIXER_LABEL_MUTED_BG,
        Color::from_hex(0x17, 0x17, 0x18)
    ),
    (
        button_primary_bg,
        BUTTON_PRIMARY_BG,
        Color::from_hex(0x00, 0x7a, 0xff)
    ),
    (
        button_primary_fg,
        BUTTON_PRIMARY_FG,
        Color::from_hex(0xf4, 0xf4, 0xf5)
    ),
    (
        button_secondary_bg,
        BUTTON_SECONDARY_BG,
        Color::from_hex(0x36, 0x38, 0x3d)
    ),
    (
        button_secondary_fg,
        BUTTON_SECONDARY_FG,
        Color::from_hex(0xf0, 0xf0, 0xf2)
    ),
    (
        button_ghost_bg,
        BUTTON_GHOST_BG,
        Color::from_hex(0x22, 0x23, 0x26)
    ),
    (
        button_ghost_fg,
        BUTTON_GHOST_FG,
        Color::from_hex(0xf0, 0xf0, 0xf2)
    ),
    (
        button_danger_bg,
        BUTTON_DANGER_BG,
        Color::from_hex(0xff, 0x3b, 0x30)
    ),
    (
        button_danger_fg,
        BUTTON_DANGER_FG,
        Color::from_hex(0xf4, 0xf4, 0xf5)
    ),
    (
        button_border,
        BUTTON_BORDER,
        Color::rgba(1.0, 1.0, 1.0, 0.18)
    ),
    (
        button_highlight,
        BUTTON_HIGHLIGHT,
        Color::rgba(1.0, 1.0, 1.0, 0.12)
    ),
    (
        button_shadow,
        BUTTON_SHADOW,
        Color::rgba(0.0, 0.0, 0.0, 0.28)
    ),
    (dropdown_bg, DROPDOWN_BG, Color::from_hex(0x41, 0x43, 0x49)),
    (dropdown_fg, DROPDOWN_FG, Color::from_hex(0xf0, 0xf0, 0xf2)),
    (
        dropdown_ring,
        DROPDOWN_RING,
        Color::from_hex(0x00, 0x7a, 0xff)
    ),
    (
        dropdown_chevron,
        DROPDOWN_CHEVRON,
        Color::from_hex(0xf4, 0xf4, 0xf5)
    ),
    (
        dropdown_badge_bg,
        DROPDOWN_BADGE_BG,
        Color::from_hex(0x00, 0x7a, 0xff)
    ),
    (
        dropdown_menu_bg,
        DROPDOWN_MENU_BG,
        Color::from_hex(0x16, 0x16, 0x18)
    ),
    (
        dropdown_menu_border,
        DROPDOWN_MENU_BORDER,
        Color::from_hex(0x46, 0x46, 0x4a)
    ),
    (
        dropdown_hover_bg,
        DROPDOWN_HOVER_BG,
        Color::from_hex(0x00, 0x5a, 0xd1)
    ),
    (
        dropdown_check,
        DROPDOWN_CHECK,
        Color::from_hex(0xf0, 0xf0, 0xf2)
    ),
    (
        dropdown_scrollbar,
        DROPDOWN_SCROLLBAR,
        Color::rgba(1.0, 1.0, 1.0, 0.25)
    ),
    (
        inspect_overlay_fill,
        INSPECT_OVERLAY_FILL,
        Color::rgba(0.18, 0.55, 1.0, 0.18)
    ),
    (
        inspect_overlay_border,
        INSPECT_OVERLAY_BORDER,
        Color::rgba(0.35, 0.75, 1.0, 0.95)
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
        widget_slider_dot,
        WIDGET_SLIDER_DOT,
        Color::from_hex(0x58, 0x58, 0x5c)
    ),
    (
        widget_knob_filled,
        WIDGET_KNOB_FILLED,
        Color::from_hex(0xc8, 0xff, 0x00)
    ),
    (
        widget_knob_track,
        WIDGET_KNOB_TRACK,
        Color::from_hex(0x08, 0x08, 0x08)
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
    (patcher_bg, PATCHER_BG, Color::from_hex(0x12, 0x12, 0x13)),
    (
        patcher_grid_minor,
        PATCHER_GRID_MINOR,
        Color::rgba(0.24, 0.24, 0.26, 0.34)
    ),
    (
        patcher_grid_major,
        PATCHER_GRID_MAJOR,
        Color::rgba(0.34, 0.34, 0.37, 0.46)
    ),
    (
        patcher_text,
        PATCHER_TEXT,
        Color::from_hex(0xe0, 0xe0, 0xe0)
    ),
    (
        patcher_text_muted,
        PATCHER_TEXT_MUTED,
        Color::from_hex(0xa8, 0xac, 0xb8)
    ),
    (
        patcher_error,
        PATCHER_ERROR,
        Color::from_hex(0xff, 0x6b, 0x73)
    ),
    (
        patcher_cable,
        PATCHER_CABLE,
        Color::rgba(0.74, 0.75, 0.84, 0.92)
    ),
    (
        patcher_feedback_cable,
        PATCHER_FEEDBACK_CABLE,
        Color::rgba(1.0, 0.59, 0.04, 0.88)
    ),
    (
        patcher_marquee_fill,
        PATCHER_MARQUEE_FILL,
        Color::rgba(0.22, 0.48, 1.0, 0.12)
    ),
    (
        patcher_marquee_border,
        PATCHER_MARQUEE_BORDER,
        Color::rgba(0.38, 0.62, 1.0, 0.72)
    ),
    (
        patcher_node_bg,
        PATCHER_NODE_BG,
        Color::from_hex(0x16, 0x16, 0x1a)
    ),
    (
        patcher_node_border,
        PATCHER_NODE_BORDER,
        Color::from_hex(0x40, 0x40, 0x4a)
    ),
    (
        patcher_node_text,
        PATCHER_NODE_TEXT,
        Color::from_hex(0x4c, 0xe0, 0x72)
    ),
    (
        patcher_node_tail_text,
        PATCHER_NODE_TAIL_TEXT,
        Color::from_hex(0xf2, 0xf2, 0xf4)
    ),
    (
        patcher_io_node_bg,
        PATCHER_IO_NODE_BG,
        Color::from_hex(0x18, 0x19, 0x1e)
    ),
    (
        patcher_io_node_border,
        PATCHER_IO_NODE_BORDER,
        Color::from_hex(0x44, 0x45, 0x50)
    ),
    (
        patcher_io_node_text,
        PATCHER_IO_NODE_TEXT,
        Color::from_hex(0x4c, 0xe0, 0x72)
    ),
    (
        patcher_param_node_bg,
        PATCHER_PARAM_NODE_BG,
        Color::from_hex(0x19, 0x19, 0x22)
    ),
    (
        patcher_param_node_border,
        PATCHER_PARAM_NODE_BORDER,
        Color::from_hex(0x3b, 0x69, 0xb1)
    ),
    (
        patcher_param_node_text,
        PATCHER_PARAM_NODE_TEXT,
        Color::from_hex(0x6d, 0xae, 0xff)
    ),
    (
        patcher_code_node_bg,
        PATCHER_CODE_NODE_BG,
        Color::from_hex(0x24, 0x16, 0x18)
    ),
    (
        patcher_code_node_border,
        PATCHER_CODE_NODE_BORDER,
        Color::from_hex(0xff, 0x5a, 0x65)
    ),
    (
        patcher_code_node_text,
        PATCHER_CODE_NODE_TEXT,
        Color::from_hex(0xff, 0x8a, 0x92)
    ),
    (
        patcher_node_hover_border,
        PATCHER_NODE_HOVER_BORDER,
        Color::from_hex(0x78, 0x7c, 0x8e)
    ),
    (
        patcher_node_selected_border,
        PATCHER_NODE_SELECTED_BORDER,
        Color::from_hex(0x4a, 0x8d, 0xff)
    ),
    (
        patcher_port_input,
        PATCHER_PORT_INPUT,
        Color::from_hex(0xff, 0xee, 0x00)
    ),
    (
        patcher_port_output,
        PATCHER_PORT_OUTPUT,
        Color::from_hex(0xff, 0x9f, 0x0a)
    ),
    (
        patcher_edit_selection,
        PATCHER_EDIT_SELECTION,
        Color::rgba(0.29, 0.55, 1.0, 0.35)
    ),
    (
        patcher_edit_cursor,
        PATCHER_EDIT_CURSOR,
        Color::from_hex(0xff, 0xff, 0xff)
    ),
    (
        patcher_alignment_guide,
        PATCHER_ALIGNMENT_GUIDE,
        Color::rgba(0.38, 0.62, 1.0, 0.86)
    ),
    (
        patcher_autocomplete_bg,
        PATCHER_AUTOCOMPLETE_BG,
        Color::rgba(0.11, 0.11, 0.14, 0.96)
    ),
    (
        patcher_autocomplete_border,
        PATCHER_AUTOCOMPLETE_BORDER,
        Color::from_hex(0x40, 0x40, 0x4a)
    ),
    (
        patcher_autocomplete_selected_bg,
        PATCHER_AUTOCOMPLETE_SELECTED_BG,
        Color::rgba(0.22, 0.39, 0.68, 0.72)
    ),
    (
        patcher_back_button_bg,
        PATCHER_BACK_BUTTON_BG,
        Color::from_hex(0x14, 0x15, 0x1a)
    ),
    (
        patcher_back_button_hover_bg,
        PATCHER_BACK_BUTTON_HOVER_BG,
        Color::from_hex(0x1e, 0x25, 0x36)
    ),
    (
        patcher_back_button_border,
        PATCHER_BACK_BUTTON_BORDER,
        Color::from_hex(0x44, 0x45, 0x50)
    ),
    (
        patcher_back_button_hover_border,
        PATCHER_BACK_BUTTON_HOVER_BORDER,
        Color::from_hex(0x6d, 0xae, 0xff)
    ),
    (
        patcher_back_button_text,
        PATCHER_BACK_BUTTON_TEXT,
        Color::from_hex(0xa8, 0xac, 0xb8)
    ),
    (
        patcher_back_button_hover_text,
        PATCHER_BACK_BUTTON_HOVER_TEXT,
        Color::from_hex(0xd7, 0xe6, 0xff)
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

pub fn generation() -> u64 {
    THEME_GENERATION.load(Ordering::Relaxed)
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
        Value::List(items) => parse_color_list(items).or_else(|| parse_color_func_call(items)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patcher_theme_slots_are_named_colors() {
        assert_eq!(named_color("patcher-bg"), Some(PATCHER_BG()));
        assert_eq!(named_color("patcher_node_text"), Some(PATCHER_NODE_TEXT()));
        assert_eq!(
            named_color("patcher back button hover border"),
            Some(PATCHER_BACK_BUTTON_HOVER_BORDER())
        );
    }

    #[test]
    fn transparent_is_a_named_color() {
        assert_eq!(
            named_color("transparent"),
            Some(Color::rgba(0.0, 0.0, 0.0, 0.0))
        );
        assert_eq!(
            parse_color_value(&Value::Keyword("none".to_string())),
            Some(Color::rgba(0.0, 0.0, 0.0, 0.0))
        );
    }
}
