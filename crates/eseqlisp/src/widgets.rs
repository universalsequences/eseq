use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::{VM, Value, format_lisp_value};

pub fn register_widget_natives(vm: &mut VM) {
    for widget in [
        "label",
        "button",
        "badge",
        "slider",
        "hslider",
        "vslider",
        "toggle",
        "knob",
        "knob-number",
        "adsr-editor",
        "meter",
        "mixer-meter",
        "modulator-curve",
        "text-input",
        "textbox",
        "number-picker",
        "number-label",
        "patcher",
        "response-curve-editor",
        "dropdown",
        "select",
        "v-stack",
        "h-stack",
        "wrap",
        "virtual-v-stack",
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

    if (widget_type == "label" || widget_type == "button" || widget_type == "badge")
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

    for (key, value) in &map {
        if matches!(&*value.borrow(), Value::ReactiveRef { .. })
            && !prop_accepts_binding(widget_type, key, &map)
        {
            return widget_diagnostic(format!(
                "{widget_type}: :{key} does not accept reactive bindings"
            ));
        }
    }

    // Text-entry and composite value widgets are always focusable.
    if widget_type == "button"
        || widget_type == "text-input"
        || widget_type == "textbox"
        || widget_type == "number-picker"
        || widget_type == "dropdown"
        || widget_type == "knob-number"
        || widget_type == "patcher"
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

fn widget_diagnostic(message: impl Into<String>) -> Value {
    let message = message.into();
    eprintln!("[eseqlisp widget diagnostic] {message}");

    let mut map = HashMap::new();
    map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::Keyword("label".to_string()))),
    );
    map.insert(
        "text".to_string(),
        Rc::new(RefCell::new(Value::String(format!("UI error: {message}")))),
    );
    map.insert(
        "color".to_string(),
        Rc::new(RefCell::new(Value::Keyword("red".to_string()))),
    );
    map.insert(
        "font-size".to_string(),
        Rc::new(RefCell::new(Value::Number(10.0))),
    );
    map.insert("wrap".to_string(), Rc::new(RefCell::new(Value::Bool(true))));
    map.insert(
        "debug-name".to_string(),
        Rc::new(RefCell::new(Value::String("widget-diagnostic".to_string()))),
    );
    map.insert(
        "__widget-diagnostic".to_string(),
        Rc::new(RefCell::new(Value::String(message))),
    );
    Value::Map(map)
}

fn prop_accepts_binding(
    widget_type: &str,
    prop: &str,
    props: &HashMap<String, Rc<RefCell<Value>>>,
) -> bool {
    if let Some(definition) = crate::widget_render::widget_definition(widget_type) {
        if definition.bindable_props().contains(&prop) {
            return true;
        }
        if widget_type == "box"
            && let Some(background) = props.get("background")
            && let Value::String(background_type) = &*background.borrow()
        {
            return crate::widget_render::sdf_widget::sdf_widget_def(background_type).is_some_and(
                |definition| definition.bindable_props.iter().any(|name| name == prop),
            );
        }
        return false;
    }
    crate::widget_render::sdf_widget::sdf_widget_def(widget_type)
        .is_some_and(|definition| definition.bindable_props.iter().any(|name| name == prop))
}
