use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::{VM, Value, format_lisp_value};

pub fn register_widget_natives(vm: &mut VM) {
    for widget in [
        "label",
        "slider",
        "hslider",
        "vslider",
        "toggle",
        "knob",
        "meter",
        "text-input",
        "select",
        "v-stack",
        "h-stack",
        "box",
        "grid",
        "tabs",
        "timeline",
        "waveform",
    ] {
        let widget_type = widget.to_string();
        vm.register_native(widget, move |args| build_widget(&widget_type, args));
    }
}

fn build_widget(widget_type: &str, args: Vec<Value>) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
    );

    let mut children = Vec::new();
    let mut i = 0;

    if widget_type == "label"
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

    if !children.is_empty() {
        map.insert(
            "children".to_string(),
            Rc::new(RefCell::new(Value::List(children))),
        );
    }

    Value::Map(map)
}
