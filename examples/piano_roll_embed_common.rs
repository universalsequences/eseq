use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use eseqlisp::backend::Backend;
use eseqlisp::frame;
#[cfg(target_os = "macos")]
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::tui;
use eseqlisp::vm::Value;
use eseqlisp::{Editor, EditorConfig, Runtime};

#[derive(Clone)]
struct Note {
    id: u64,
    pitch: usize,
    start: f64,
    end: f64,
    selected: bool,
}

#[derive(Clone)]
struct DraftNote {
    pitch: usize,
    start: f64,
    end: f64,
}

struct PatternHost {
    notes: Vec<Note>,
    draft: Option<DraftNote>,
    next_note_id: u64,
    playing: bool,
    playhead_time: f64,
    tempo: f64,
    loop_start: f64,
    loop_end: f64,
    view_start: f64,
    view_duration: f64,
    lane_scroll: f64,
    tool: String,
    cursor_time: f64,
    dirty: bool,
    status: String,
}

#[allow(dead_code)]
pub fn run_tui() -> io::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        default_hook(info);
    }));

    let (mut editor, host) = bootstrap_editor();

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let mut last_tick = Instant::now();

    loop {
        let now = Instant::now();
        let delta = now.saturating_duration_since(last_tick);
        last_tick = now;
        host.borrow_mut().tick(delta);

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => editor.handle_key(key),
                Event::Mouse(mouse) => {
                    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                    editor.handle_mouse(
                        mouse,
                        1,
                        1,
                        cols.saturating_sub(2),
                        rows.saturating_sub(3),
                    );
                }
                Event::Resize(_, _) => editor.mark_needs_redraw(),
                _ => {}
            }
        }

        sync_pattern_state(&mut editor, &host);

        if editor.needs_redraw() {
            terminal.draw(|f| {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                let viewport_width = (cols as usize).saturating_sub(2);
                let viewport_height = (rows as usize).saturating_sub(3);
                let render_frame =
                    frame::build_render_frame(&mut editor, viewport_width, viewport_height);
                tui::render(f, &render_frame);
            })?;
            editor.clear_needs_redraw();
        }

        if editor.should_quit() {
            break;
        }
    }

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn run_metal() -> Result<(), eseqlisp::backend::BackendError> {
    let (mut editor, host) = bootstrap_editor();
    let mut backend = MetalBackend::new()?;
    backend.initialize()?;
    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut last_tick = Instant::now();
    let mut pending_drag: Option<(Event, (f32, f32))> = None;

    loop {
        let now = Instant::now();
        let delta = now.saturating_duration_since(last_tick);
        last_tick = now;
        host.borrow_mut().tick(delta);

        let (cols, rows) = backend.viewport_size();
        let redraw_pending = editor.needs_redraw() || pending_drag.is_some();
        let timeout = if redraw_pending {
            frame_interval.saturating_sub(last_render_at.elapsed())
        } else {
            Duration::from_millis(16)
        };

        match backend.poll_event(timeout) {
            Some(Event::Key(key)) => editor.handle_key(key),
            Some(Event::Mouse(mouse)) => {
                let (precise_col, precise_row) = backend
                    .take_last_precise_mouse()
                    .unwrap_or((mouse.column as f32, mouse.row as f32));
                if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
                    pending_drag = Some((Event::Mouse(mouse), (precise_col, precise_row)));
                } else {
                    editor.handle_mouse_precise(
                        mouse,
                        0,
                        0,
                        cols as u16,
                        rows.saturating_sub(1) as u16,
                        precise_col,
                        precise_row,
                    );
                }
            }
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_touchpad_magnify(0, 0, precise_col, precise_row, delta);
        }

        while let Some(((delta_x, delta_y), (precise_col, precise_row))) =
            backend.take_pending_scroll()
        {
            editor.handle_touchpad_scroll(0, 0, precise_col, precise_row, delta_x, delta_y);
        }

        if last_render_at.elapsed() >= frame_interval {
            if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
                editor.handle_mouse_precise(
                    mouse,
                    0,
                    0,
                    cols as u16,
                    rows.saturating_sub(1) as u16,
                    precise_col,
                    precise_row,
                );
            }
        }

        sync_pattern_state(&mut editor, &host);

        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let render_frame = frame::build_render_frame(&mut editor, cols, rows.saturating_sub(1));
            backend.render(&render_frame)?;
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
        }

        if editor.should_quit() {
            break;
        }
    }

    backend.teardown()
}

fn bootstrap_editor() -> (Editor, Rc<RefCell<PatternHost>>) {
    let init_src = std::fs::read_to_string("init.lisp").unwrap_or_default();
    let host = Rc::new(RefCell::new(PatternHost::new()));

    let mut runtime = Runtime::new();
    runtime.register_reactive("PATTERN", pattern_reactive_fields(&host.borrow()), false);
    register_pattern_natives(&mut runtime, host.clone());

    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            ..EditorConfig::default()
        },
    );
    let _ = editor.open_scratch_buffer("*piano-roll*", piano_roll_buffer_text());
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    sync_pattern_state(&mut editor, &host);

    (editor, host)
}

fn register_pattern_natives(runtime: &mut Runtime, host: Rc<RefCell<PatternHost>>) {
    runtime.register_native("pattern-set-tool", {
        let host = host.clone();
        move |args, ctx| {
            let Some(tool) = args.first().and_then(as_keyword_or_string) else {
                return Err("pattern-set-tool expects a keyword or string".to_string());
            };
            host.borrow_mut().set_tool(&tool);
            ctx.set_status(format!("tool -> {tool}"));
            Ok(Value::Keyword(tool))
        }
    });

    runtime.register_native("pattern-toggle-play", {
        let host = host.clone();
        move |_args, ctx| {
            let playing = host.borrow_mut().toggle_play();
            ctx.set_status(if playing { "play" } else { "stop" });
            Ok(Value::Bool(playing))
        }
    });

    runtime.register_native("pattern-rewind", {
        let host = host.clone();
        move |_args, ctx| {
            host.borrow_mut().rewind();
            ctx.set_status("rewind");
            Ok(Value::Bool(true))
        }
    });

    runtime.register_native("pattern-handle-action", move |args, ctx| {
        let Some(action) = args.first().cloned() else {
            return Err("pattern-handle-action expects an action map".to_string());
        };
        let status = host.borrow_mut().apply_action(&action)?;
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });
}

fn sync_pattern_state(editor: &mut Editor, host: &Rc<RefCell<PatternHost>>) {
    if !host.borrow().dirty {
        return;
    }

    let mut host = host.borrow_mut();
    let runtime = editor.runtime_mut();
    runtime.set_reactive("PATTERN", "lanes", host.lanes_value());
    runtime.set_reactive("PATTERN", "items", host.items_value());
    runtime.set_reactive("PATTERN", "selection", host.selection_value());
    runtime.set_reactive("PATTERN", "playing", Value::Bool(host.playing));
    runtime.set_reactive(
        "PATTERN",
        "playhead_time",
        Value::Number(host.playhead_time),
    );
    runtime.set_reactive("PATTERN", "tempo", Value::Number(host.tempo));
    runtime.set_reactive("PATTERN", "view_start", Value::Number(host.view_start));
    runtime.set_reactive(
        "PATTERN",
        "view_duration",
        Value::Number(host.view_duration),
    );
    runtime.set_reactive("PATTERN", "lane_scroll", Value::Number(host.lane_scroll));
    runtime.set_reactive("PATTERN", "tool", Value::Keyword(host.tool.clone()));
    runtime.set_reactive("PATTERN", "cursor_time", Value::Number(host.cursor_time));
    runtime.set_reactive("PATTERN", "status", Value::String(host.status.clone()));
    runtime.run_reactive_cycle();
    host.dirty = false;
    editor.mark_needs_redraw();
}

fn pattern_reactive_fields(host: &PatternHost) -> Vec<(&'static str, Value)> {
    vec![
        ("lanes", host.lanes_value()),
        ("items", host.items_value()),
        ("selection", host.selection_value()),
        ("playing", Value::Bool(host.playing)),
        ("playhead_time", Value::Number(host.playhead_time)),
        ("tempo", Value::Number(host.tempo)),
        ("view_start", Value::Number(host.view_start)),
        ("view_duration", Value::Number(host.view_duration)),
        ("lane_scroll", Value::Number(host.lane_scroll)),
        ("tool", Value::Keyword(host.tool.clone())),
        ("cursor_time", Value::Number(host.cursor_time)),
        ("status", Value::String(host.status.clone())),
    ]
}

impl PatternHost {
    fn new() -> Self {
        let mut notes = Vec::new();
        let pitches = [36, 40, 43, 47, 48, 52, 55, 59, 60, 64, 67, 71];
        for (idx, pitch) in pitches.into_iter().enumerate() {
            notes.push(Note {
                id: idx as u64 + 1,
                pitch,
                start: (idx as f64) * 2.0,
                end: (idx as f64) * 2.0 + 2.0 + (idx % 3) as f64,
                selected: pitch == 60,
            });
        }

        Self {
            notes,
            draft: None,
            next_note_id: 1000,
            playing: false,
            playhead_time: 0.0,
            tempo: 120.0,
            loop_start: 0.0,
            loop_end: 32.0,
            view_start: 0.0,
            view_duration: 32.0,
            lane_scroll: 20.0,
            tool: "draw".to_string(),
            cursor_time: 0.0,
            dirty: true,
            status: "host piano roll ready".to_string(),
        }
    }

    fn set_tool(&mut self, tool: &str) {
        self.tool = tool.to_string();
        self.status = format!("tool: {}", self.tool);
        self.dirty = true;
    }

    fn toggle_play(&mut self) -> bool {
        self.playing = !self.playing;
        self.status = if self.playing {
            format!("play {:.1} bpm", self.tempo)
        } else {
            format!("stop at {:.1}", self.playhead_time)
        };
        self.dirty = true;
        self.playing
    }

    fn rewind(&mut self) {
        self.playhead_time = self.loop_start;
        self.status = "rewind".to_string();
        self.dirty = true;
    }

    fn tick(&mut self, delta: Duration) {
        if !self.playing {
            return;
        }
        let beats = delta.as_secs_f64() * self.tempo / 60.0;
        if beats <= 0.0 {
            return;
        }
        self.playhead_time += beats;
        if self.playhead_time >= self.loop_end {
            let loop_len = (self.loop_end - self.loop_start).max(1.0);
            self.playhead_time =
                self.loop_start + (self.playhead_time - self.loop_start).rem_euclid(loop_len);
        }
        self.dirty = true;
    }

    fn apply_action(&mut self, action: &Value) -> Result<String, String> {
        let action = expect_map(action)?;
        let action_type = action
            .get("type")
            .and_then(as_keyword_or_string)
            .ok_or_else(|| "action is missing :type".to_string())?;

        match action_type.as_str() {
            "select" => {
                let ids = parse_id_list(action.get("ids"))?;
                self.select_only(&ids);
                self.status = format!("selected {} note(s)", ids.len());
            }
            "clear-selection" => {
                self.clear_selection();
                self.status = "selection cleared".to_string();
            }
            "set-tool" => {
                let tool = action
                    .get("tool")
                    .and_then(as_keyword_or_string)
                    .ok_or_else(|| "set-tool missing tool".to_string())?;
                self.set_tool(&tool);
            }
            "marquee-select" => {
                let time_a = action.get("time-a").and_then(as_number).unwrap_or(0.0);
                let time_b = action.get("time-b").and_then(as_number).unwrap_or(0.0);
                let lane_a = action.get("lane-a").and_then(as_usize).unwrap_or(0);
                let lane_b = action.get("lane-b").and_then(as_usize).unwrap_or(0);
                let selected = self
                    .notes
                    .iter()
                    .filter(|note| {
                        note.pitch >= lane_a
                            && note.pitch <= lane_b
                            && note.start < time_b
                            && note.end > time_a
                    })
                    .map(|note| note.id)
                    .collect::<Vec<_>>();
                self.select_only(&selected);
                self.status = format!("marquee selected {} note(s)", selected.len());
            }
            "delete-items" => {
                let ids = parse_id_list(action.get("ids"))?;
                let wanted = ids.iter().copied().collect::<HashSet<_>>();
                self.notes.retain(|note| !wanted.contains(&note.id));
                self.status = format!("deleted {} note(s)", ids.len());
            }
            "nudge-selection" => {
                let ids = parse_id_list(action.get("ids"))?;
                let delta_time = action.get("delta-time").and_then(as_number).unwrap_or(0.0);
                let delta_lane = action.get("delta-lane").and_then(as_number).unwrap_or(0.0);
                let wanted = ids.iter().copied().collect::<HashSet<_>>();
                for note in &mut self.notes {
                    if wanted.contains(&note.id) {
                        let duration = (note.end - note.start).max(1.0);
                        note.start = (note.start + delta_time).max(0.0);
                        note.end = note.start + duration;
                        note.pitch = ((note.pitch as f64 + delta_lane).round() as isize)
                            .clamp(0, 87) as usize;
                    }
                }
                self.status = format!("nudged {} note(s)", ids.len());
            }
            "move-items-absolute" => {
                let ids = parse_id_list(action.get("ids"))?;
                let anchor_id = action
                    .get("anchor-id")
                    .and_then(as_u64)
                    .ok_or_else(|| "move-items-absolute missing anchor-id".to_string())?;
                let start = action.get("start").and_then(as_number).unwrap_or(0.0);
                let lane = action.get("lane").and_then(as_usize).unwrap_or(0);
                self.move_items_absolute(&ids, anchor_id, start, lane);
                self.status = format!("moved {} note(s)", ids.len());
            }
            "resize-item-absolute" => {
                let id = action
                    .get("id")
                    .and_then(as_u64)
                    .ok_or_else(|| "resize-item-absolute missing id".to_string())?;
                let edge = action
                    .get("edge")
                    .and_then(as_keyword_or_string)
                    .unwrap_or_else(|| "end".to_string());
                let time = action.get("time").and_then(as_number).unwrap_or(0.0);
                self.resize_item(id, &edge, time);
                self.status = format!("resized note {id}");
            }
            "create-item" => {
                let pitch = action.get("lane").and_then(as_usize).unwrap_or(0);
                let start = action.get("start").and_then(as_number).unwrap_or(0.0);
                let end = action.get("end").and_then(as_number).unwrap_or(start + 1.0);
                self.clear_selection();
                self.draft = Some(DraftNote {
                    pitch,
                    start,
                    end: end.max(start + 1.0),
                });
                self.status = "drawing note".to_string();
            }
            "finish-create-item" => {
                let pitch = action.get("lane").and_then(as_usize).unwrap_or(0);
                let start = action.get("start").and_then(as_number).unwrap_or(0.0);
                let end = action.get("end").and_then(as_number).unwrap_or(start + 1.0);
                self.finish_create_item(pitch, start, end.max(start + 1.0));
                self.status = "created note".to_string();
            }
            "scroll-view" => {
                let delta_time = action.get("delta-time").and_then(as_number).unwrap_or(0.0);
                let delta_lanes = action.get("delta-lanes").and_then(as_number).unwrap_or(0.0);
                self.view_start = (self.view_start + delta_time).max(0.0);
                self.lane_scroll = (self.lane_scroll + delta_lanes).clamp(0.0, 87.0);
                self.status = format!(
                    "view {}..{} lanes {}",
                    self.view_start.round(),
                    (self.view_start + self.view_duration).round(),
                    self.lane_scroll.round()
                );
            }
            "zoom-view" => {
                let anchor = action.get("anchor-time").and_then(as_number).unwrap_or(0.0);
                let factor = action.get("factor").and_then(as_number).unwrap_or(1.0);
                let next_duration = (self.view_duration / factor).clamp(8.0, 128.0);
                if (next_duration - self.view_duration).abs() < f64::EPSILON {
                    return Ok(self.status.clone());
                }
                let anchor_ratio = if self.view_duration <= 0.0 {
                    0.5
                } else {
                    ((anchor - self.view_start) / self.view_duration).clamp(0.0, 1.0)
                };
                self.view_start = (anchor - next_duration * anchor_ratio).max(0.0);
                self.view_duration = next_duration;
                self.status = format!("zoom {:.1} beats", self.view_duration);
            }
            "set-cursor" => {
                self.cursor_time = action
                    .get("time")
                    .and_then(as_number)
                    .unwrap_or(self.cursor_time);
                self.status = format!("cursor {:.1}", self.cursor_time);
            }
            other => {
                self.status = format!("ignored action {other}");
            }
        }

        self.dirty = true;
        Ok(self.status.clone())
    }

    fn move_items_absolute(&mut self, ids: &[u64], anchor_id: u64, start: f64, lane: usize) {
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        let Some(anchor) = self.notes.iter().find(|note| note.id == anchor_id).cloned() else {
            return;
        };
        for note in &mut self.notes {
            if !wanted.contains(&note.id) {
                continue;
            }
            let duration = (note.end - note.start).max(1.0);
            let start_offset = note.start - anchor.start;
            let lane_offset = note.pitch as isize - anchor.pitch as isize;
            note.start = (start + start_offset).max(0.0);
            note.end = note.start + duration;
            note.pitch = (lane as isize + lane_offset).clamp(0, 87) as usize;
        }
    }

    fn resize_item(&mut self, id: u64, edge: &str, time: f64) {
        let Some(note) = self.notes.iter_mut().find(|note| note.id == id) else {
            return;
        };
        if edge == "start" {
            note.start = time.clamp(0.0, note.end.max(1.0) - 1.0);
        } else {
            note.end = time.max(note.start + 1.0);
        }
    }

    fn finish_create_item(&mut self, pitch: usize, start: f64, end: f64) {
        self.clear_selection();
        let id = self.next_note_id;
        self.next_note_id += 1;
        self.notes.push(Note {
            id,
            pitch: pitch.clamp(0, 87),
            start,
            end,
            selected: true,
        });
        self.draft = None;
    }

    fn clear_selection(&mut self) {
        for note in &mut self.notes {
            note.selected = false;
        }
    }

    fn select_only(&mut self, ids: &[u64]) {
        let wanted = ids.iter().copied().collect::<HashSet<_>>();
        for note in &mut self.notes {
            note.selected = wanted.contains(&note.id);
        }
    }

    fn lanes_value(&self) -> Value {
        let mut lanes = Vec::with_capacity(88);
        for pitch in 0..88 {
            let midi = 21 + pitch as i32;
            let is_black = is_black_key(midi);
            lanes.push(map_value(vec![
                ("id", Value::Number(pitch as f64)),
                ("label", Value::String(String::new())),
                (
                    "sidebar-bg",
                    Value::Keyword(if is_black { "gray" } else { "white" }.to_string()),
                ),
                (
                    "label-fg",
                    Value::Keyword(if is_black { "white" } else { "black" }.to_string()),
                ),
            ]));
        }
        list_value(lanes)
    }

    fn items_value(&self) -> Value {
        let mut items = self
            .notes
            .iter()
            .map(|note| {
                map_value(vec![
                    ("id", Value::Number(note.id as f64)),
                    ("lane", Value::Number(note.pitch as f64)),
                    ("start", Value::Number(note.start)),
                    ("end", Value::Number(note.end)),
                    ("selected", Value::Bool(note.selected)),
                    ("color", Value::Keyword("white".to_string())),
                ])
            })
            .collect::<Vec<_>>();
        if let Some(draft) = &self.draft {
            items.push(map_value(vec![
                ("id", Value::Number(-1.0)),
                ("lane", Value::Number(draft.pitch as f64)),
                ("start", Value::Number(draft.start)),
                ("end", Value::Number(draft.end)),
                ("selected", Value::Bool(true)),
                ("color", Value::Keyword("green".to_string())),
            ]));
        }
        list_value(items)
    }

    fn selection_value(&self) -> Value {
        list_value(
            self.notes
                .iter()
                .filter(|note| note.selected)
                .map(|note| Value::Number(note.id as f64))
                .collect(),
        )
    }
}

fn piano_roll_buffer_text() -> &'static str {
    r#"; Host-owned piano roll embedding demo.
; Rust owns PATTERN state and handles every timeline action.
; Evaluate this buffer again with C-x C-b after edits.

(effect
  (v-stack
    (label "host piano roll / reactive embedding stress test" :color :yellow)
    (label PATTERN.status :color :dim)
    (h-stack
      (label (if PATTERN.playing "[stop]" "[play]")
        :focusable true
        :on-enter (lambda () (pattern-toggle-play)))
      (label "[rewind]"
        :focusable true
        :on-enter (lambda () (pattern-rewind)))
      (label (if (= PATTERN.tool :pointer) "[pointer]" " pointer ")
        :focusable true
        :on-enter (lambda () (pattern-set-tool :pointer)))
      (label (if (= PATTERN.tool :draw) "[draw]" " draw ")
        :focusable true
        :on-enter (lambda () (pattern-set-tool :draw)))
      (label (if (= PATTERN.tool :erase) "[erase]" " erase ")
        :focusable true
        :on-enter (lambda () (pattern-set-tool :erase)))
      (label (fmt " playhead {:.1}" PATTERN.playhead_time) :color :yellow)
      (label (fmt " cursor {:.1}" PATTERN.cursor_time) :color :cyan))
    (timeline
      :height 26
      :focusable true
      :sidebar-width 6
      :time-ruler (dict :mode :bars-beats :beats-per-bar 4)
      :tool PATTERN.tool
      :playhead-time PATTERN.playhead_time
      :lanes PATTERN.lanes
      :items PATTERN.items
      :selection PATTERN.selection
      :view-start PATTERN.view_start
      :view-duration PATTERN.view_duration
      :lane-scroll PATTERN.lane_scroll
      :snap 1
      :on-action |event| (pattern-handle-action event))))"#
}

fn expect_map(value: &Value) -> Result<std::collections::HashMap<String, Value>, String> {
    let Value::Map(map) = value else {
        return Err("expected map".to_string());
    };
    Ok(map
        .iter()
        .map(|(key, value)| (key.clone(), value.borrow().clone()))
        .collect())
}

fn parse_id_list(value: Option<&Value>) -> Result<Vec<u64>, String> {
    let Some(Value::List(items)) = value else {
        return Ok(vec![]);
    };
    items
        .iter()
        .map(|item| as_u64(&item.borrow()).ok_or_else(|| "expected numeric id".to_string()))
        .collect()
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

fn as_u64(value: &Value) -> Option<u64> {
    as_number(value).and_then(|n| (n >= 0.0).then_some(n as u64))
}

fn as_usize(value: &Value) -> Option<usize> {
    as_number(value).map(|n| n.max(0.0) as usize)
}

fn as_keyword_or_string(value: &Value) -> Option<String> {
    match value {
        Value::Keyword(s) | Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

fn list_value(items: Vec<Value>) -> Value {
    Value::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

fn is_black_key(midi_note: i32) -> bool {
    matches!(midi_note.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}
