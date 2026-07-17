#![allow(
    dead_code,
    private_interfaces,
    unused_variables,
    clippy::arc_with_non_send_sync,
    clippy::bool_to_int_with_if,
    clippy::collapsible_else_if,
    clippy::collapsible_if,
    clippy::declare_interior_mutable_const,
    clippy::empty_line_after_doc_comments,
    clippy::excessive_precision,
    clippy::filter_map_bool_then,
    clippy::if_same_then_else,
    clippy::inspect_for_each,
    clippy::int_plus_one,
    clippy::io_other_error,
    clippy::items_after_test_module,
    clippy::len_without_is_empty,
    clippy::manual_clamp,
    clippy::manual_ignore_case_cmp,
    clippy::manual_is_multiple_of,
    clippy::manual_memcpy,
    clippy::manual_ok_err,
    clippy::manual_range_contains,
    clippy::map_flatten,
    clippy::missing_safety_doc,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_range_loop,
    clippy::needless_return,
    clippy::new_without_default,
    clippy::not_unsafe_ptr_arg_deref,
    clippy::result_large_err,
    clippy::single_match,
    clippy::single_element_loop,
    clippy::too_many_arguments,
    clippy::transmute_ptr_to_ptr,
    clippy::missing_transmute_annotations,
    clippy::borrow_deref_ref,
    clippy::type_complexity,
    clippy::useless_conversion,
    clippy::useless_format
)]

mod accumulator;
#[allow(dead_code)]
pub mod agent;
pub mod analysis;
pub mod audio;
pub mod audio_tap;
pub mod audiograph;
pub mod crash;
#[allow(dead_code)]
pub mod effects;
pub mod generator;
pub mod graph;
#[allow(dead_code)]
pub mod lisp_host;
pub mod macro_engine;
pub mod mixer_volume;
pub mod neural;
pub mod paths;
pub mod plock_variants;
pub mod process;
pub mod project;
pub mod quantized_launch;
pub mod recorder;
pub mod sample_db;
pub mod sample_import;
pub mod sampler;
mod scale;
mod scheduled_event;
mod scheduler;
#[allow(dead_code)]
pub mod sequencer;
pub mod track_color;
#[allow(dead_code)]
pub mod track_modulator;
pub mod tui;
#[allow(dead_code)]
mod voice;
#[allow(dead_code)]
pub mod voice_modulator;
pub mod warp_grid;

pub mod engine;
