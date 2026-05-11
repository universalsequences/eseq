use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::{VM, Value, format_lisp_value};

pub fn register_widget_natives(vm: &mut VM) {
    for widget in [
        "label",
        "button",
        "slider",
        "hslider",
        "vslider",
        "toggle",
        "knob",
        "knob-number",
        "adsr-editor",
        "meter",
        "mixer-meter",
        "text-input",
        "number-picker",
        "number-label",
        "response-curve-editor",
        "dropdown",
        "select",
        "v-stack",
        "h-stack",
        "box",
        "grid",
        "responsive-grid",
        "image",
        "tabs",
        "timeline",
        "transport-clock",
        "waveform",
        "scroll",
        "tree",
    ] {
        let widget_type = widget.to_string();
        vm.register_native(widget, move |args| build_widget(&widget_type, args));
    }
}

pub fn build_widget(widget_type: &str, args: Vec<Value>) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
    );

    let mut children = Vec::new();
    let mut i = 0;

    if (widget_type == "label" || widget_type == "button")
        && let Some(value) = args.first()
    {
        let text = match value {
            Value::String(text) => text.clone(),
            other => format_lisp_value(other),
        };
        map.insert(
            "text".to_string(),
            Rc::new(RefCell::new(Value::String(text))),
        );
        i = 1;
    }

    while i < args.len() {
        match args.get(i) {
            Some(Value::Keyword(key)) if i + 1 < args.len() => {
                if matches!(args.get(i + 1), Some(Value::ReactiveRef { .. }))
                    && !prop_accepts_binding(widget_type, key)
                {
                    return Value::String(format!(
                        "{widget_type}: :{key} does not accept reactive bindings"
                    ));
                }
                map.insert(key.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
                i += 2;
            }
            Some(Value::Map(widget_map)) if widget_map.contains_key("type") => {
                children.push(Rc::new(RefCell::new(args[i].clone())));
                i += 1;
            }
            Some(Value::List(items)) => {
                for item in items {
                    if let Value::Map(widget_map) = &*item.borrow()
                        && widget_map.contains_key("type")
                    {
                        children.push(Rc::new(RefCell::new(item.borrow().clone())));
                    }
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Text-entry and composite value widgets are always focusable.
    if widget_type == "button"
        || widget_type == "text-input"
        || widget_type == "number-picker"
        || widget_type == "dropdown"
        || widget_type == "knob-number"
    {
        map.entry("focusable".to_string())
            .or_insert_with(|| Rc::new(RefCell::new(Value::Bool(true))));
    }

    if !children.is_empty() {
        map.insert(
            "children".to_string(),
            Rc::new(RefCell::new(Value::List(children))),
        );
    }

    Value::Map(map)
}

fn prop_accepts_binding(widget_type: &str, prop: &str) -> bool {
    if let Some(definition) = crate::widget_render::widget_definition(widget_type) {
        return definition.bindable_props().contains(&prop);
    }
    crate::widget_render::sdf_widget::sdf_widget_def(widget_type)
        .is_some_and(|definition| definition.bindable_props.iter().any(|name| name == prop))
}
