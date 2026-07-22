use super::*;

pub(super) fn lisp_string(value: impl Into<String>) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::String(value.into())))
}

pub(super) fn lisp_number(value: f64) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::Number(value)))
}

pub(super) fn lisp_bool(value: bool) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(EValue::Bool(value)))
}

pub(super) fn lisp_value(value: EValue) -> Rc<RefCell<EValue>> {
    Rc::new(RefCell::new(value))
}

pub(super) fn lisp_list(items: Vec<EValue>) -> EValue {
    EValue::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

pub(super) fn step_snapshot_to_value(step: usize, snapshot: StepSnapshot) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("step".to_string(), lisp_number(step as f64));
    map.insert("active".to_string(), lisp_bool(snapshot.active));
    map.insert(
        "duration".to_string(),
        lisp_number(snapshot.params[StepParam::Duration.index()] as f64),
    );
    map.insert(
        "velocity".to_string(),
        lisp_number(snapshot.params[StepParam::Velocity.index()] as f64),
    );
    map.insert(
        "speed".to_string(),
        lisp_number(snapshot.params[StepParam::Speed.index()] as f64),
    );
    map.insert(
        "transpose".to_string(),
        lisp_number(snapshot.params[StepParam::Transpose.index()] as f64),
    );
    map.insert(
        "pan".to_string(),
        lisp_number(snapshot.params[StepParam::Pan.index()] as f64),
    );
    map.insert(
        "delay".to_string(),
        lisp_number(snapshot.params[StepParam::Delay.index()] as f64),
    );
    map.insert(
        "chord".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord
                .into_iter()
                .map(|note| EValue::Number(note as f64))
                .collect(),
        )),
    );
    map.insert(
        "chord-durations".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord_durations
                .into_iter()
                .map(|duration| EValue::Number(duration as f64))
                .collect(),
        )),
    );
    map.insert(
        "chord-delays".to_string(),
        lisp_value(lisp_list(
            snapshot
                .chord_delays
                .into_iter()
                .map(|delay| EValue::Number(delay as f64))
                .collect(),
        )),
    );
    EValue::Map(map)
}
