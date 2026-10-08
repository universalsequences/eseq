//! The *groove* buffer's host side (docs/rack-groove-spec.md, "UI"): the
//! Extract Groove modal's payload and the delete confirmations. The buffer
//! itself, driven end to end through the production host path:
//! `host_kinds::tests::rack_view`.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn extract_modal_payload_maps_to_the_extract_request() {
    // Pure payload -> request mapping of the Extract Groove modal.
    let payload = eseqlisp::vm::Value::Map(
        [
            ("group-id", Value::Number(1.0)),
            ("name", Value::String("  Pocket ".into())),
            ("bars", Value::Number(2.0)),
            ("resolution", Value::String("1/32".into())),
            ("quantize", Value::Bool(false)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
        .collect(),
    );
    let request = super::extract_request_from_payload(&payload).expect("request");
    assert_eq!(request.options.name, "Pocket");
    assert_eq!(request.options.period_beats, 8.0);
    assert_eq!(request.options.resolution_beats, 0.125);
    assert!(!request.quantize_source);
}

#[test]
fn groove_confirm_messages_list_racks() {
    assert_eq!(
        super::delete_pool_groove_confirm_message("Take", &["Kit A".to_string()]),
        "Delete groove 'Take'? Kit A plays it; it will play straight (undo restores it)."
    );
    assert_eq!(
        super::delete_pool_groove_confirm_message(
            "Take",
            &["Kit A".into(), "Kit B".into(), "Kit C".into()]
        ),
        "Delete groove 'Take'? Kit A, Kit B and Kit C play it; they will play straight (undo restores it)."
    );
}
