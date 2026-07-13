use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eseqlisp::vm::Value;

pub(crate) fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

pub(crate) fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut map = HashMap::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value_cell(value));
    }
    Value::Map(map)
}

pub(crate) fn list_value(values: impl IntoIterator<Item = Value>) -> Value {
    Value::List(values.into_iter().map(value_cell).collect())
}

pub(crate) fn build_string_list(items: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = items
        .iter()
        .map(|item| Rc::new(RefCell::new(Value::String(item.clone()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_flat_tree_items(items: &[String]) -> Value {
    build_tree_items(items, None)
}

pub(crate) fn build_icon_tree_items(items: &[String], icon: &str) -> Value {
    build_tree_items(items, Some(icon))
}

fn build_tree_items(items: &[String], icon: Option<&str>) -> Value {
    use std::collections::HashMap;
    let items: Vec<Rc<RefCell<Value>>> = items
        .iter()
        .map(|item| {
            let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            map.insert(
                "label".to_string(),
                Rc::new(RefCell::new(Value::String(item.clone()))),
            );
            if let Some(icon) = icon {
                map.insert(
                    "icon".to_string(),
                    Rc::new(RefCell::new(Value::Keyword(icon.to_string()))),
                );
            }
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(items)
}
