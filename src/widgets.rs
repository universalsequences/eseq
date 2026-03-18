use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::{VM, Value};

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
        && let Some(Value::String(text)) = args.first()
    {
        map.insert(
            "text".to_string(),
            Rc::new(RefCell::new(Value::String(text.clone()))),
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
