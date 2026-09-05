use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::{
    INLINE_ANCHOR_PROP, INLINE_PLACEMENT_PROP, INLINE_WRITEBACK_CALLBACK, SOURCE_MODULE_PATH_PROP,
    SOURCE_SYMBOL_PROP, VM, Value, format_lisp_value,
};

pub const BUILTIN_WIDGET_NAMES: &[&str] = &[
    "label",
    "lane-preview",
    "button",
    "badge",
    "slider",
    "hslider",
    "vslider",
    "toggle",
    "event-view",
    "piano-keyboard",
    "linegraph",
    "matrix",
    "knob",
    "knob-number",
    "adsr-editor",
    "meter",
    "mixer-meter",
    "modulator-curve",
    "lfo-curve",
    "text-input",
    "textbox",
    "number-picker",
    "number-label",
    "patcher",
    "response-curve-editor",
    "eq8-editor",
    "dropdown",
    "menu-button",
    "select",
    "v-stack",
    "h-stack",
    "wrap",
    "virtual-v-stack",
    "box",
    "modal",
    "context-menu",
    "menu-item",
    "menu-separator",
    "grid",
    "responsive-grid",
    "image",
    "tabs",
    "timeline",
    "transport-clock",
    "waveform",
    "drift-waveform",
    "wavetable-viewer",
    "sound-glyph",
    "spectrogram",
    "scope",
    "multiband-meter",
    "compressor-display",
    "phaser-notch",
    "roar-shaper",
    "roar-filter",
    "gate-led",
    "scroll",
    "tree",
    "xy-pad",
];

pub fn is_builtin_widget_name(name: &str) -> bool {
    BUILTIN_WIDGET_NAMES.contains(&name)
}

pub fn register_widget_natives(vm: &mut VM) {
    for widget in BUILTIN_WIDGET_NAMES {
        let widget_type = widget.to_string();
        vm.register_native_with_vm(widget, move |args, vm| {
            let mut widget = build_widget(&widget_type, args);
            vm.qualify_widget_stable_key(&mut widget);
            if let Some(symbol) = vm.current_source_symbol() {
                if let Value::Map(map) = &mut widget {
                    map.insert(
                        SOURCE_SYMBOL_PROP.to_string(),
                        Rc::new(RefCell::new(Value::String(symbol))),
                    );
                }
            }
            if let Some(module) = vm.current_source_file() {
                if let Value::Map(map) = &mut widget {
                    map.insert(
                        SOURCE_MODULE_PATH_PROP.to_string(),
                        Rc::new(RefCell::new(Value::String(module.display().to_string()))),
                    );
                }
            }
            widget
        });
    }
    register_inline_widget_target_binding_native(vm);
    register_inline_value_widget_natives(vm);
    register_inline_scope_native(vm);
    register_inline_lane_native(vm);
}

fn register_inline_widget_target_binding_native(vm: &mut VM) {
    vm.register_native_with_vm("__bind-inline-widget-target", |args, vm| {
        if !vm.inline_widget_registration_enabled() {
            return Value::Bool(true);
        }
        let [
            Value::String(revision),
            Value::Number(start_byte),
            Value::Number(end_byte),
            Value::String(inlet),
            target,
        ] = args.as_slice()
        else {
            return Value::Bool(false);
        };
        if !start_byte.is_finite()
            || *start_byte < 0.0
            || start_byte.fract() != 0.0
            || !end_byte.is_finite()
            || *end_byte < *start_byte
            || end_byte.fract() != 0.0
        {
            return Value::Bool(false);
        }
        Value::Bool(vm.attach_inline_widget_runtime_target_by_source_identity(
            revision.clone(),
            *start_byte as usize,
            *end_byte as usize,
            inlet,
            target.clone(),
        ))
    });
}

fn register_inline_value_widget_natives(vm: &mut VM) {
    for (form_name, widget_type, default_range) in [
        ("~slider", "hslider", Some((0.0, 1.0))),
        ("~knob", "inline-knob", Some((0.0, 1.0))),
        ("~toggle", "toggle", None),
    ] {
        let widget_type = widget_type.to_string();
        vm.register_native_with_vm(form_name, move |args, vm| {
            let value = args.first().cloned().unwrap_or(Value::Nil);
            if !vm.inline_widget_registration_enabled() {
                return value;
            }

            let parent_callee = keyword_string_arg(&args, crate::vm::INLINE_PARENT_CALLEE_PROP);
            let parent_inlet = keyword_string_arg(&args, crate::vm::INLINE_PARENT_INLET_PROP);
            let inferred = parent_callee
                .as_deref()
                .zip(parent_inlet.as_deref())
                .and_then(|(callee, inlet)| vm.resolve_inline_widget_metadata(callee, inlet));
            let display_value = if form_name == "~toggle" {
                match &value {
                    Value::Number(number) => Value::Bool(*number != 0.0),
                    _ => value.clone(),
                }
            } else {
                value.clone()
            };
            let mut widget_args = vec![Value::Keyword("value".to_string()), display_value];
            let mut property_index = 1usize;
            while property_index + 1 < args.len() {
                if !matches!(&args[property_index], Value::Keyword(key) if key == "chan") {
                    widget_args.push(args[property_index].clone());
                    widget_args.push(args[property_index + 1].clone());
                }
                property_index += 2;
            }
            if let Some((default_min, default_max)) = default_range {
                if !args
                    .iter()
                    .any(|arg| matches!(arg, Value::Keyword(key) if key == "min"))
                {
                    let min = inferred
                        .and_then(|metadata| metadata.min)
                        .unwrap_or(default_min);
                    widget_args.extend([Value::Keyword("min".to_string()), Value::Number(min)]);
                }
                if !args
                    .iter()
                    .any(|arg| matches!(arg, Value::Keyword(key) if key == "max"))
                {
                    let max = inferred
                        .and_then(|metadata| metadata.max)
                        .unwrap_or(default_max);
                    widget_args.extend([Value::Keyword("max".to_string()), Value::Number(max)]);
                }
            }
            if !args
                .iter()
                .any(|arg| matches!(arg, Value::Keyword(key) if key == "step"))
                && let Some(step) = inferred.and_then(|metadata| metadata.step)
            {
                widget_args.extend([Value::Keyword("step".to_string()), Value::Number(step)]);
            }
            widget_args.extend([
                Value::Keyword("on-change".to_string()),
                Value::String(INLINE_WRITEBACK_CALLBACK.to_string()),
            ]);
            let mut widget = build_widget(&widget_type, widget_args);
            if let Value::Map(map) = &mut widget {
                let mut index = 1usize;
                while index + 1 < args.len() {
                    if let Value::Keyword(key) = &args[index]
                        && (key.starts_with("__source-") || key.starts_with("__inline-value-"))
                    {
                        map.insert(key.clone(), Rc::new(RefCell::new(args[index + 1].clone())));
                    }
                    index += 2;
                }
                map.insert(
                    INLINE_ANCHOR_PROP.to_string(),
                    Rc::new(RefCell::new(Value::Bool(true))),
                );
                if form_name == "~toggle" && matches!(value, Value::Number(_)) {
                    map.insert(
                        "__inline-toggle-numeric".to_string(),
                        Rc::new(RefCell::new(Value::Bool(true))),
                    );
                }
                map.insert(
                    "__inline-text-value".to_string(),
                    Rc::new(RefCell::new(value.clone())),
                );
                map.insert(
                    INLINE_PLACEMENT_PROP.to_string(),
                    Rc::new(RefCell::new(Value::Keyword("inline".to_string()))),
                );
                let inline_width = match form_name {
                    "~slider" => 8.0,
                    "~knob" => 3.0,
                    "~toggle" => 5.0,
                    _ => 4.0,
                };
                map.insert(
                    "__inline-width".to_string(),
                    Rc::new(RefCell::new(Value::Number(inline_width))),
                );
            }
            vm.register_inline_widget(widget);
            value
        });
    }
}

fn keyword_string_arg(args: &[Value], key: &str) -> Option<String> {
    args.windows(2).find_map(|pair| match pair {
        [Value::Keyword(current), Value::String(value)] if current == key => Some(value.clone()),
        _ => None,
    })
}

fn register_inline_scope_native(vm: &mut VM) {
    vm.register_native_with_vm("~scope", |args, vm| {
        if !vm.inline_widget_registration_enabled() {
            return Value::Nil;
        }
        let track = args.windows(2).find_map(|pair| match pair {
            [Value::Keyword(key), Value::Number(index)] if key == "track" && *index >= 0.0 => {
                Some(*index)
            }
            _ => None,
        });
        let mut widget_args = Vec::new();
        let mut index = 0usize;
        while index + 1 < args.len() {
            if !matches!(&args[index], Value::Keyword(key) if key == "track") {
                widget_args.push(args[index].clone());
                widget_args.push(args[index + 1].clone());
            }
            index += 2;
        }
        if !widget_args
            .iter()
            .any(|arg| matches!(arg, Value::Keyword(key) if key == "source"))
        {
            let source = if let Some(track) = track {
                Value::Map(HashMap::from([
                    (
                        "kind".to_string(),
                        Rc::new(RefCell::new(Value::Keyword("track".to_string()))),
                    ),
                    (
                        "index".to_string(),
                        Rc::new(RefCell::new(Value::Number(track))),
                    ),
                ]))
            } else {
                Value::Keyword("master".to_string())
            };
            widget_args.extend([Value::Keyword("source".to_string()), source]);
        }
        if !widget_args
            .iter()
            .any(|arg| matches!(arg, Value::Keyword(key) if key == "height"))
        {
            widget_args.extend([Value::Keyword("height".to_string()), Value::Number(6.0)]);
        }
        let mut widget = build_widget("scope", widget_args);
        if let Value::Map(map) = &mut widget {
            map.insert(
                INLINE_ANCHOR_PROP.to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
            map.insert(
                INLINE_PLACEMENT_PROP.to_string(),
                Rc::new(RefCell::new(Value::Keyword("band".to_string()))),
            );
        }
        vm.register_inline_widget(widget);
        Value::Nil
    });
}

fn register_inline_lane_native(vm: &mut VM) {
    vm.register_native_with_vm("~lane", |args, vm| {
        let value = args.first().cloned().unwrap_or(Value::Nil);
        if !vm.inline_widget_registration_enabled() {
            return value;
        }
        let mut widget_args = vec![Value::Keyword("values".to_string()), value.clone()];
        widget_args.extend(args.iter().skip(1).cloned());
        if !widget_args
            .iter()
            .any(|arg| matches!(arg, Value::Keyword(key) if key == "height"))
        {
            widget_args.extend([Value::Keyword("height".to_string()), Value::Number(4.0)]);
        }
        let mut widget = build_widget("lane-preview", widget_args);
        if let Value::Map(map) = &mut widget {
            map.insert(
                INLINE_ANCHOR_PROP.to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
            map.insert(
                INLINE_PLACEMENT_PROP.to_string(),
                Rc::new(RefCell::new(Value::Keyword("band".to_string()))),
            );
        }
        vm.register_inline_widget(widget);
        value
    });
}

pub fn build_widget(widget_type: &str, args: Vec<Value>) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
    );

    let mut children = Vec::new();
    let mut i = 0;

    if (widget_type == "label"
        || widget_type == "button"
        || widget_type == "badge"
        || widget_type == "menu-item")
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
        || widget_type == "menu-button"
        || widget_type == "knob-number"
        || widget_type == "patcher"
    {
        map.entry("focusable".to_string())
            .or_insert_with(|| Rc::new(RefCell::new(Value::Bool(true))));
    }

    if widget_type == "menu-button" {
        map.insert(
            "action-menu".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
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
