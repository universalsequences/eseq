use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "toggle-bus-step",
    "set-bus-step-active",
    "set-bus-step-param",
    "set-selected-bus-step-param",
    "select-bus-step-range",
    "select-bus-step",
    "select-all-bus-steps",
    "delete-selected-bus-steps",
    "move-bus-step-drag",
    "shift-selected-bus-steps",
    "set-bus-sequencer-param",
];

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let selected_steps = ctx.shared.selected_steps.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    match name {
        "toggle-bus-step" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let step = map_number(map, "step").map(|value| value as usize);
                if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        bus.gate_sequence.toggle_step(step);
                        app.publish_bus_gate_runtime();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-bus-step-active" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let step = map_number(map, "step").map(|value| value as usize);
                let active = map_bool(map, "active");
                if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        if let Some(slot) = bus.gate_sequence.steps.get_mut(step) {
                            if *slot != active {
                                *slot = active;
                                app.publish_bus_gate_runtime();
                                let rt = editor.runtime_mut();
                                sync_bus_mixer_state(rt, &app);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        }
        "set-bus-step-param" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let step = map_number(map, "step").map(|value| value as usize);
                let param = map_string(map, "param");
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(bus_idx), Some(step), Some(param), Some(value)) =
                    (bus_idx, step, param, value)
                {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        match param.as_str() {
                            "duration" | "dur" => {
                                bus.gate_sequence.set_step_duration(step, value);
                            }
                            "sync" | "syn" => {
                                bus.gate_sequence.set_step_sync(step, value);
                            }
                            _ => {
                                bus.gate_sequence.set_step_velocity(step, value);
                            }
                        }
                        app.publish_bus_gate_runtime();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-selected-bus-step-param" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let param = map_string(map, "param");
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(bus_idx), Some(param), Some(value)) =
                    (bus_idx, param, value)
                {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        let steps: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        for step in steps {
                            if step >= bus.gate_sequence.num_steps {
                                continue;
                            }
                            match param.as_str() {
                                "duration" | "dur" => {
                                    bus.gate_sequence.set_step_duration(step, value);
                                }
                                "sync" | "syn" => {
                                    bus.gate_sequence.set_step_sync(step, value);
                                }
                                _ => {
                                    bus.gate_sequence.set_step_velocity(step, value);
                                }
                            }
                        }
                        app.publish_bus_gate_runtime();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "select-bus-step-range" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let start = map_number(map, "start").map(|value| value as usize);
                let end = map_number(map, "end").map(|value| value as usize);
                if let (Some(bus_idx), Some(start), Some(end)) = (bus_idx, start, end) {
                    if let Some(bus) = app.buses.get(bus_idx) {
                        let num_steps = bus.gate_sequence.num_steps.max(1);
                        let a = start.min(num_steps - 1);
                        let b = end.min(num_steps - 1);
                        let lo = a.min(b);
                        let hi = a.max(b);
                        {
                            let mut set = selected_steps.lock().unwrap();
                            set.clear();
                            set.extend(lo..=hi);
                        }
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "select-bus-step" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let step = map_number(map, "step").map(|value| value as usize);
                if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                    if let Some(bus) = app.buses.get(bus_idx) {
                        let num_steps = bus.gate_sequence.num_steps.max(1);
                        let step = step.min(num_steps - 1);
                        {
                            let mut set = selected_steps.lock().unwrap();
                            if !set.insert(step) {
                                set.remove(&step);
                            }
                        }
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "select-all-bus-steps" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                if let Some(bus_idx) = bus_idx {
                    if let Some(bus) = app.buses.get(bus_idx) {
                        let mut set = selected_steps.lock().unwrap();
                        set.clear();
                        set.extend(0..bus.gate_sequence.num_steps);
                        drop(set);
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "delete-selected-bus-steps" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                if let Some(bus_idx) = bus_idx {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        let steps: Vec<usize> = {
                            let mut set = selected_steps.lock().unwrap();
                            let mut steps: Vec<usize> = set.iter().copied().collect();
                            steps.sort_unstable();
                            set.clear();
                            steps
                        };
                        for step in steps {
                            if step >= bus.gate_sequence.num_steps {
                                continue;
                            }
                            bus.gate_sequence.steps[step] = false;
                            bus.gate_sequence.velocities[step] = 1.0;
                            bus.gate_sequence.durations[step] = 1.0;
                            bus.gate_sequence.syncs[step] = 0.0;
                            bus.gate_sequence.timebase_plocks[step] = None;
                            bus.gate_sequence.swing_plocks[step] = None;
                            bus.gate_sequence.swing_resolution_plocks[step] = None;
                            for slot in &mut bus.effect_slots {
                                if let Some(step_plocks) = slot.plocks.get_mut(step) {
                                    for value in step_plocks {
                                        *value = None;
                                    }
                                }
                            }
                        }
                        app.publish_bus_gate_runtime();
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "move-bus-step-drag" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let start = map_number(map, "start").map(|value| value as usize);
                let target = map_number(map, "target").map(|value| value as usize);
                if let (Some(bus_idx), Some(start), Some(target)) =
                    (bus_idx, start, target)
                {
                    if start != target {
                        if let Some(bus) = app.buses.get_mut(bus_idx) {
                            let num_steps = bus.gate_sequence.num_steps;
                            if start < num_steps && target < num_steps {
                                let delta = target as isize - start as isize;
                                let mut move_selection = false;
                                let steps: Vec<usize> = {
                                    let set = selected_steps.lock().unwrap();
                                    if set.contains(&start) {
                                        move_selection = true;
                                        let mut steps: Vec<usize> =
                                            set.iter().copied().collect();
                                        steps.sort_unstable();
                                        steps
                                    } else {
                                        vec![start]
                                    }
                                };
                                if let (Some(&first), Some(&last)) =
                                    (steps.first(), steps.last())
                                {
                                    let new_first = first as isize + delta;
                                    let new_last = last as isize + delta;
                                    if new_first >= 0 && new_last < num_steps as isize {
                                        let snapshots: Vec<_> = steps
                                            .iter()
                                            .map(|&step| {
                                                (
                                                    step,
                                                    bus.gate_sequence.steps[step],
                                                    bus.gate_sequence.velocities[step],
                                                    bus.gate_sequence.durations[step],
                                                    bus.gate_sequence.syncs[step],
                                                    bus.gate_sequence.timebase_plocks
                                                        [step],
                                                    bus.gate_sequence.swing_plocks
                                                        [step],
                                                    bus.gate_sequence
                                                        .swing_resolution_plocks[step],
                                                    bus.effect_slots
                                                        .iter()
                                                        .map(|slot| {
                                                            slot.plocks
                                                                .get(step)
                                                                .cloned()
                                                                .unwrap_or_default()
                                                        })
                                                        .collect::<Vec<_>>(),
                                                )
                                            })
                                            .collect();
                                        for &step in &steps {
                                            bus.gate_sequence.steps[step] = false;
                                            bus.gate_sequence.velocities[step] = 1.0;
                                            bus.gate_sequence.durations[step] = 1.0;
                                            bus.gate_sequence.syncs[step] = 0.0;
                                            bus.gate_sequence.timebase_plocks[step] =
                                                None;
                                            bus.gate_sequence.swing_plocks[step] = None;
                                            bus.gate_sequence.swing_resolution_plocks
                                                [step] = None;
                                            for slot in &mut bus.effect_slots {
                                                if let Some(step_plocks) =
                                                    slot.plocks.get_mut(step)
                                                {
                                                    for value in step_plocks {
                                                        *value = None;
                                                    }
                                                }
                                            }
                                        }
                                        let moved_steps: Vec<usize> = snapshots
                                            .iter()
                                            .map(|(step, ..)| {
                                                (*step as isize + delta) as usize
                                            })
                                            .collect();
                                        for (snapshot, dst_step) in snapshots
                                            .iter()
                                            .zip(moved_steps.iter().copied())
                                        {
                                            bus.gate_sequence.steps[dst_step] =
                                                snapshot.1;
                                            bus.gate_sequence.velocities[dst_step] =
                                                snapshot.2;
                                            bus.gate_sequence.durations[dst_step] =
                                                snapshot.3;
                                            bus.gate_sequence.syncs[dst_step] =
                                                snapshot.4;
                                            bus.gate_sequence.timebase_plocks
                                                [dst_step] = snapshot.5;
                                            bus.gate_sequence.swing_plocks[dst_step] =
                                                snapshot.6;
                                            bus.gate_sequence.swing_resolution_plocks
                                                [dst_step] = snapshot.7;
                                            for (slot_idx, slot_plocks) in
                                                snapshot.8.iter().enumerate()
                                            {
                                                let Some(slot) =
                                                    bus.effect_slots.get_mut(slot_idx)
                                                else {
                                                    continue;
                                                };
                                                let Some(dst_plocks) =
                                                    slot.plocks.get_mut(dst_step)
                                                else {
                                                    continue;
                                                };
                                                for (param_idx, value) in
                                                    slot_plocks.iter().enumerate()
                                                {
                                                    if param_idx < dst_plocks.len() {
                                                        dst_plocks[param_idx] = *value;
                                                    }
                                                }
                                            }
                                        }
                                        if move_selection {
                                            let mut set =
                                                selected_steps.lock().unwrap();
                                            set.clear();
                                            set.extend(moved_steps);
                                        }
                                        app.publish_bus_gate_runtime();
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "selected-steps",
                                            build_selection_value(&selected_steps),
                                        );
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "shift-selected-bus-steps" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let direction =
                    map_number(map, "direction").map(|value| value.signum() as isize);
                if let (Some(bus_idx), Some(delta)) = (bus_idx, direction) {
                    if delta != 0 {
                        if let Some(bus) = app.buses.get_mut(bus_idx) {
                            let steps: Vec<usize> = {
                                let set = selected_steps.lock().unwrap();
                                let mut steps: Vec<usize> =
                                    set.iter().copied().collect();
                                steps.sort_unstable();
                                steps
                            };
                            if let (Some(&first), Some(&last)) =
                                (steps.first(), steps.last())
                            {
                                let num_steps = bus.gate_sequence.num_steps;
                                let can_shift = if delta < 0 {
                                    first > 0
                                } else {
                                    last + 1 < num_steps
                                };
                                if can_shift {
                                    let snapshots: Vec<_> = steps
                                        .iter()
                                        .map(|&step| {
                                            (
                                                step,
                                                bus.gate_sequence.steps[step],
                                                bus.gate_sequence.velocities[step],
                                                bus.gate_sequence.durations[step],
                                                bus.gate_sequence.syncs[step],
                                                bus.gate_sequence.timebase_plocks[step],
                                                bus.gate_sequence.swing_plocks[step],
                                                bus.gate_sequence
                                                    .swing_resolution_plocks[step],
                                                bus.effect_slots
                                                    .iter()
                                                    .map(|slot| {
                                                        slot.plocks
                                                            .get(step)
                                                            .cloned()
                                                            .unwrap_or_default()
                                                    })
                                                    .collect::<Vec<_>>(),
                                            )
                                        })
                                        .collect();
                                    for &step in &steps {
                                        bus.gate_sequence.steps[step] = false;
                                        bus.gate_sequence.velocities[step] = 1.0;
                                        bus.gate_sequence.durations[step] = 1.0;
                                        bus.gate_sequence.syncs[step] = 0.0;
                                        bus.gate_sequence.timebase_plocks[step] = None;
                                        bus.gate_sequence.swing_plocks[step] = None;
                                        bus.gate_sequence.swing_resolution_plocks
                                            [step] = None;
                                        for slot in &mut bus.effect_slots {
                                            if let Some(step_plocks) =
                                                slot.plocks.get_mut(step)
                                            {
                                                for value in step_plocks {
                                                    *value = None;
                                                }
                                            }
                                        }
                                    }
                                    let shifted_steps: Vec<usize> = snapshots
                                        .iter()
                                        .map(|(step, ..)| {
                                            (*step as isize + delta) as usize
                                        })
                                        .collect();
                                    for (snapshot, dst_step) in snapshots
                                        .iter()
                                        .zip(shifted_steps.iter().copied())
                                    {
                                        bus.gate_sequence.steps[dst_step] = snapshot.1;
                                        bus.gate_sequence.velocities[dst_step] =
                                            snapshot.2;
                                        bus.gate_sequence.durations[dst_step] =
                                            snapshot.3;
                                        bus.gate_sequence.syncs[dst_step] = snapshot.4;
                                        bus.gate_sequence.timebase_plocks[dst_step] =
                                            snapshot.5;
                                        bus.gate_sequence.swing_plocks[dst_step] =
                                            snapshot.6;
                                        bus.gate_sequence.swing_resolution_plocks
                                            [dst_step] = snapshot.7;
                                        for (slot_idx, slot_plocks) in
                                            snapshot.8.iter().enumerate()
                                        {
                                            let Some(slot) =
                                                bus.effect_slots.get_mut(slot_idx)
                                            else {
                                                continue;
                                            };
                                            let Some(dst_plocks) =
                                                slot.plocks.get_mut(dst_step)
                                            else {
                                                continue;
                                            };
                                            for (param_idx, value) in
                                                slot_plocks.iter().enumerate()
                                            {
                                                if param_idx < dst_plocks.len() {
                                                    dst_plocks[param_idx] = *value;
                                                }
                                            }
                                        }
                                    }
                                    {
                                        let mut set = selected_steps.lock().unwrap();
                                        set.clear();
                                        set.extend(shifted_steps);
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            }
        }
        "set-bus-sequencer-param" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map_number(map, "bus").map(|value| value as usize);
                let param = map_string(map, "param");
                let value = map_number(map, "value").map(|value| value as f32);
                let label = map_string(map, "label");
                if let (Some(bus_idx), Some(param)) = (bus_idx, param) {
                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                        let selected_bus_steps: Vec<usize> = selected_steps
                            .lock()
                            .unwrap()
                            .iter()
                            .copied()
                            .filter(|step| *step < bus.gate_sequence.num_steps)
                            .collect();
                        let write_plock =
                            !selected_bus_steps.is_empty() && param != "num-steps";
                        match param.as_str() {
                            "num-steps" => {
                                if let Some(value) = value {
                                    bus.gate_sequence.set_num_steps(value as usize);
                                }
                            }
                            "swing" => {
                                if let Some(value) = value {
                                    let swing = value.clamp(50.0, 75.0);
                                    if write_plock {
                                        for step in &selected_bus_steps {
                                            bus.gate_sequence.swing_plocks[*step] =
                                                Some(swing);
                                        }
                                    } else {
                                        bus.gate_sequence.swing = swing;
                                    }
                                }
                            }
                            "timebase" => {
                                if let Some(label) = label {
                                    let normalized = label.to_ascii_lowercase();
                                    if let Some(idx) =
                                        Timebase::LABELS.iter().position(|candidate| {
                                            candidate.to_ascii_lowercase() == normalized
                                        })
                                    {
                                        let timebase = Timebase::ALL[idx];
                                        if write_plock {
                                            for step in &selected_bus_steps {
                                                bus.gate_sequence.timebase_plocks
                                                    [*step] = Some(timebase);
                                            }
                                        } else {
                                            bus.gate_sequence.timebase = timebase;
                                        }
                                    }
                                }
                            }
                            "swing-resolution" => {
                                if let Some(label) = label {
                                    let normalized = label.to_ascii_lowercase();
                                    if let Some(idx) = SwingResolution::LABELS
                                        .iter()
                                        .position(|candidate| {
                                            candidate.to_ascii_lowercase() == normalized
                                        })
                                    {
                                        let resolution = SwingResolution::ALL[idx];
                                        if write_plock {
                                            for step in &selected_bus_steps {
                                                bus.gate_sequence
                                                    .swing_resolution_plocks[*step] =
                                                    Some(resolution);
                                            }
                                        } else {
                                            bus.gate_sequence.swing_resolution =
                                                resolution;
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                        app.publish_bus_gate_runtime();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        _ => {}
    }
}
