use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use eseqlisp::backend::{Backend, BackendError};
use eseqlisp::editor::ViewMode;
#[cfg(target_os = "macos")]
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::vm::Value;
use eseqlisp::{Editor, EditorConfig, Runtime};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

type SharedValue = Rc<RefCell<Value>>;

#[derive(Clone)]
struct Track {
    title: String,
    path: PathBuf,
    cover_path: Option<PathBuf>,
    duration: Option<Duration>,
}

struct AlbumGroup<'a> {
    label: String,
    path: String,
    cover_path: Option<PathBuf>,
    tracks: Vec<(usize, &'a Track)>,
}

struct PlayerHost {
    tracks: Vec<Track>,
    current: Option<usize>,
    playing: bool,
    volume: f32,
    status: String,
    dirty: bool,
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    started_at: Option<Instant>,
    paused_at: Duration,
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    std::env::set_current_dir(musicplayer_dir().ok_or("musicplayer crate directory not found")?)
        .map_err(|e| format!("failed to enter musicplayer crate directory: {e}"))?;
    let (mut editor, host) = bootstrap_editor();
    let mut backend =
        MetalBackend::new_with_size(1100, 700).map_err(|_| "Metal backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "Metal backend initialization failed")?;
    backend.set_image_decode_min_interval(Duration::from_millis(8));

    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        let (cols, rows) = backend.viewport_size();
        editor.update_tile_rects(cols as u16, rows as u16);
        for name in ["*library*", "*album*", "*now-playing*"] {
            editor.refresh_visible_layouts_for_buffer_named(name);
        }
    }

    run_metal_loop(&mut editor, &mut backend, host).map_err(|_| "render loop failed")?;
    backend
        .teardown()
        .map_err(|_| "Metal backend teardown failed")?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("musicplayer's Metal UI is macOS only.");
}

#[cfg(target_os = "macos")]
fn run_metal_loop(
    editor: &mut Editor,
    backend: &mut MetalBackend,
    host: Rc<RefCell<PlayerHost>>,
) -> Result<(), BackendError> {
    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let player_ui_interval = Duration::from_secs(1);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut last_player_ui_sync = Instant::now() - player_ui_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;

    loop {
        editor.update_timers();
        host.borrow_mut().poll_end_of_track();

        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);

        let should_sync_player = host.borrow().dirty
            || (host.borrow().playing && last_player_ui_sync.elapsed() >= player_ui_interval);
        if should_sync_player {
            sync_player_state(editor, &host);
            last_player_ui_sync = Instant::now();
        }
        if host.borrow().playing {
            editor.mark_needs_redraw();
        }

        let timeout = if editor.needs_redraw() {
            frame_interval.saturating_sub(last_render_at.elapsed())
        } else if host.borrow().playing {
            player_ui_interval.saturating_sub(last_player_ui_sync.elapsed())
        } else {
            Duration::from_millis(32)
        };

        match backend.poll_event(timeout) {
            Some(Event::Key(key)) => {
                if key.kind == KeyEventKind::Press && handle_player_shortcut(editor, &host, key) {
                    continue;
                }
                if key.kind == KeyEventKind::Press {
                    editor.handle_key(key);
                }
            }
            Some(Event::Mouse(mouse)) => {
                let (precise_col, precise_row) = backend
                    .take_last_precise_mouse()
                    .unwrap_or((mouse.column as f32, mouse.row as f32));
                if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
                    pending_drag = Some((Event::Mouse(mouse), (precise_col, precise_row)));
                } else {
                    if matches!(mouse.kind, MouseEventKind::Up(_)) {
                        pending_drag = None;
                    }
                    editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
                }
            }
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_tiled_touchpad_magnify(precise_col, precise_row, 0, delta);
        }

        while let Some(((delta_x, delta_y), (precise_col, precise_row))) =
            backend.take_pending_scroll()
        {
            let handled =
                editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, delta_x, delta_y);
            if handled {
                continue;
            }

            if editor.is_ui_scroll_mode() {
                editor.apply_smooth_widget_scroll(delta_x * 0.05, delta_y * 0.05);
                continue;
            }

            scroll_accum_y += delta_y;
            let line_px = backend.viewport_size().1.max(1) as f32 / (rows.max(1) as f32);
            let threshold = line_px.max(20.0);
            while scroll_accum_y > threshold {
                scroll_accum_y -= threshold;
                editor.handle_tiled_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::ScrollUp,
                        column: precise_col as u16,
                        row: precise_row as u16,
                        modifiers: KeyModifiers::NONE,
                    },
                    precise_col,
                    precise_row,
                    0,
                );
            }
            while scroll_accum_y < -threshold {
                scroll_accum_y += threshold;
                editor.handle_tiled_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::ScrollDown,
                        column: precise_col as u16,
                        row: precise_row as u16,
                        modifiers: KeyModifiers::NONE,
                    },
                    precise_col,
                    precise_row,
                    0,
                );
            }

            scroll_accum_x += delta_x;
            while scroll_accum_x > threshold {
                scroll_accum_x -= threshold;
                editor.handle_tiled_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::ScrollLeft,
                        column: precise_col as u16,
                        row: precise_row as u16,
                        modifiers: KeyModifiers::NONE,
                    },
                    precise_col,
                    precise_row,
                    0,
                );
            }
            while scroll_accum_x < -threshold {
                scroll_accum_x += threshold;
                editor.handle_tiled_mouse_precise(
                    MouseEvent {
                        kind: MouseEventKind::ScrollRight,
                        column: precise_col as u16,
                        row: precise_row as u16,
                        modifiers: KeyModifiers::NONE,
                    },
                    precise_col,
                    precise_row,
                    0,
                );
            }
        }

        if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
            editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
        }

        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let frame = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
            backend.render_tiled(&frame)?;
            editor.clear_needs_redraw();
            if backend.take_pending_image_loads() {
                editor.mark_needs_redraw();
            }
            last_render_at = Instant::now();
        }

        if editor.should_quit() {
            break;
        }
    }

    Ok(())
}

fn bootstrap_editor() -> (Editor, Rc<RefCell<PlayerHost>>) {
    let init_src = read_eseqlisp_init_source();
    let host = Rc::new(RefCell::new(PlayerHost::new()));

    let mut runtime = Runtime::new();
    runtime.register_reactive("MP", player_reactive_fields(&host.borrow()), false);
    register_player_natives(&mut runtime, host.clone());

    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            init_source_path: None,
            vim_mode: true,
        },
    );
    let _ = editor.open_or_create_file_buffer("music-player.lisp");
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    activate_player_ui_buffer(&mut editor);
    sync_player_state(&mut editor, &host);

    (editor, host)
}

fn read_eseqlisp_init_source() -> String {
    eseqlisp_init_candidates()
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .unwrap_or_default()
}

fn musicplayer_dir() -> Option<PathBuf> {
    find_package_dir(
        "musicplayer",
        "MUSICPLAYER_ROOT",
        env!("CARGO_MANIFEST_DIR"),
    )
}

fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(root) = std::env::var("ESEQLISP_ROOT") {
        if !root.trim().is_empty() {
            paths.push(PathBuf::from(root).join("init.lisp"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("../eseqlisp/init.lisp"));
        paths.push(cwd.join("crates/eseqlisp/init.lisp"));
    }
    for root in repo_roots_from_current_exe() {
        paths.push(root.join("crates/eseqlisp/init.lisp"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../eseqlisp/init.lisp"));
    paths.push(PathBuf::from("../eseqlisp/init.lisp"));
    paths.push(PathBuf::from("init.lisp"));
    paths
}

fn find_package_dir(package: &str, env_var: &str, compile_manifest_dir: &str) -> Option<PathBuf> {
    if let Ok(root) = std::env::var(env_var) {
        if !root.trim().is_empty() {
            let path = PathBuf::from(root);
            if is_package_dir(&path, package) {
                return Some(path);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if is_package_dir(&cwd, package) {
            return Some(cwd);
        }
        let workspace_member = cwd.join("crates").join(package);
        if is_package_dir(&workspace_member, package) {
            return Some(workspace_member);
        }
    }

    for root in repo_roots_from_current_exe() {
        let workspace_member = root.join("crates").join(package);
        if is_package_dir(&workspace_member, package) {
            return Some(workspace_member);
        }
    }

    let fallback = PathBuf::from(compile_manifest_dir);
    is_package_dir(&fallback, package).then_some(fallback)
}

fn repo_roots_from_current_exe() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    exe.ancestors()
        .filter(|path| path.join("crates").is_dir())
        .map(Path::to_path_buf)
        .collect()
}

fn is_package_dir(path: &Path, package: &str) -> bool {
    path.join("Cargo.toml").is_file() && path.file_name().is_some_and(|name| name == package)
}

fn activate_player_ui_buffer(editor: &mut Editor) {
    let mut library_id = None;
    for name in ["*library*", "*album*", "*now-playing*"] {
        let Some(buffer_id) = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == name)
            .map(|buffer| buffer.id)
        else {
            continue;
        };
        editor.set_active_buffer(buffer_id);
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.active_buffer_mut().read_only = true;
        if name == "*library*" {
            library_id = Some(buffer_id);
        }
    }

    if let Some(buffer_id) = library_id {
        editor.set_active_buffer(buffer_id);
    }
}

fn active_buffer_is_player_transport(editor: &Editor) -> bool {
    editor.active_buffer().name == "*now-playing*"
}

fn register_player_natives(runtime: &mut Runtime, host: Rc<RefCell<PlayerHost>>) {
    runtime.register_native("mp-toggle-play", {
        let host = host.clone();
        move |_args, ctx| {
            let status = host.borrow_mut().toggle_play();
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-play-track", {
        let host = host.clone();
        move |args, ctx| {
            let idx = usize_arg(&args, 0, "mp-play-track expects an index")?;
            let status = host.borrow_mut().play_track(idx);
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-play-album", {
        let host = host.clone();
        move |args, ctx| {
            let idx = usize_arg(&args, 0, "mp-play-album expects a track index")?;
            let status = host.borrow_mut().play_track(idx);
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-next", {
        let host = host.clone();
        move |_args, ctx| {
            let status = host.borrow_mut().next_track();
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-prev", {
        let host = host.clone();
        move |_args, ctx| {
            let status = host.borrow_mut().prev_track();
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-set-volume", {
        let host = host.clone();
        move |args, ctx| {
            let value = number_arg(&args, 0, "mp-set-volume expects a value")? as f32;
            let volume = host.borrow_mut().set_volume(value);
            ctx.set_status(format!("volume {:.0}%", volume * 100.0));
            Ok(Value::Number(volume as f64))
        }
    });

    runtime.register_native("mp-seek", {
        let host = host.clone();
        move |args, ctx| {
            let seconds = number_arg(&args, 0, "mp-seek expects seconds")?;
            let status = host
                .borrow_mut()
                .seek_to(Duration::from_secs_f64(seconds.max(0.0)));
            ctx.set_status(status.clone());
            Ok(Value::String(status))
        }
    });

    runtime.register_native("mp-rescan", move |_args, ctx| {
        let count = host.borrow_mut().rescan();
        let status = format!("found {count} track(s)");
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });
}

fn handle_player_shortcut(
    editor: &mut Editor,
    host: &Rc<RefCell<PlayerHost>>,
    key: KeyEvent,
) -> bool {
    if !active_buffer_is_player_transport(editor) {
        return false;
    }

    match key.code {
        KeyCode::Char(' ') => {
            let _ = host.borrow_mut().toggle_play();
            editor.mark_needs_redraw();
            true
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = host.borrow_mut().next_track();
            editor.mark_needs_redraw();
            true
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = host.borrow_mut().prev_track();
            editor.mark_needs_redraw();
            true
        }
        _ => false,
    }
}

fn sync_player_state(editor: &mut Editor, host: &Rc<RefCell<PlayerHost>>) {
    if !host.borrow().dirty && !host.borrow().playing {
        return;
    }

    let mut host = host.borrow_mut();
    let runtime = editor.runtime_mut();
    let full_sync = host.dirty;
    if full_sync {
        runtime.set_reactive("MP", "tracks", host.tracks_value());
        runtime.set_reactive("MP", "albums", host.albums_value());
        runtime.set_reactive("MP", "library_tree", host.library_tree_value());
        runtime.set_reactive("MP", "current_index", current_index_value(host.current));
        runtime.set_reactive(
            "MP",
            "current_album_path",
            Value::String(host.current_album_path()),
        );
        runtime.set_reactive(
            "MP",
            "current_album_label",
            Value::String(host.current_album_label()),
        );
        runtime.set_reactive(
            "MP",
            "current_album_tracks",
            host.current_album_tracks_value(),
        );
        runtime.set_reactive("MP", "current_path", Value::String(host.current_path()));
        runtime.set_reactive(
            "MP",
            "current_cover_path",
            Value::String(host.current_cover_path()),
        );
        runtime.set_reactive("MP", "current_title", Value::String(host.current_title()));
        runtime.set_reactive("MP", "playing", Value::Bool(host.playing));
        runtime.set_reactive("MP", "volume", Value::Number(host.volume as f64));
        runtime.set_reactive(
            "MP",
            "duration",
            Value::Number(host.current_duration().as_secs_f64()),
        );
        runtime.set_reactive("MP", "status", Value::String(host.status.clone()));
    }
    runtime.set_reactive(
        "MP",
        "position",
        Value::Number(host.position().as_secs_f64()),
    );
    runtime.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.refresh_visible_layouts_for_buffer_named("*now-playing*");
    host.dirty = false;
    editor.mark_needs_redraw();
}

fn player_reactive_fields(host: &PlayerHost) -> Vec<(&'static str, Value)> {
    vec![
        ("tracks", host.tracks_value()),
        ("albums", host.albums_value()),
        ("library_tree", host.library_tree_value()),
        ("current_index", current_index_value(host.current)),
        (
            "current_album_path",
            Value::String(host.current_album_path()),
        ),
        (
            "current_album_label",
            Value::String(host.current_album_label()),
        ),
        ("current_album_tracks", host.current_album_tracks_value()),
        ("current_path", Value::String(host.current_path())),
        (
            "current_cover_path",
            Value::String(host.current_cover_path()),
        ),
        ("current_title", Value::String(host.current_title())),
        ("playing", Value::Bool(host.playing)),
        ("volume", Value::Number(host.volume as f64)),
        ("position", Value::Number(host.position().as_secs_f64())),
        (
            "duration",
            Value::Number(host.current_duration().as_secs_f64()),
        ),
        ("status", Value::String(host.status.clone())),
    ]
}

impl PlayerHost {
    fn new() -> Self {
        let (stream, handle, status) = match OutputStream::try_default() {
            Ok((stream, handle)) => (Some(stream), Some(handle), "audio ready".to_string()),
            Err(err) => (None, None, format!("audio unavailable: {err}")),
        };

        let mut host = Self {
            tracks: Vec::new(),
            current: None,
            playing: false,
            volume: 0.8,
            status,
            dirty: true,
            _stream: stream,
            handle,
            sink: None,
            started_at: None,
            paused_at: Duration::ZERO,
        };
        host.rescan();
        host
    }

    fn rescan(&mut self) -> usize {
        self.tracks = scan_music_files();
        if self.tracks.is_empty() {
            self.current = None;
            self.status = "drop audio files in ./Music, then rescan".to_string();
        } else if self.current.is_none_or(|idx| idx >= self.tracks.len()) {
            self.current = Some(0);
            self.status = format!("found {} track(s)", self.tracks.len());
        } else {
            self.status = format!("found {} track(s)", self.tracks.len());
        }
        self.dirty = true;
        self.tracks.len()
    }

    fn play_track(&mut self, idx: usize) -> String {
        if idx >= self.tracks.len() {
            self.status = format!("track {idx} out of range");
            self.dirty = true;
            return self.status.clone();
        }
        self.current = Some(idx);
        self.paused_at = Duration::ZERO;
        self.start_current();
        self.status.clone()
    }

    fn toggle_play(&mut self) -> String {
        if self.sink.is_none() {
            if self.current.is_none() && !self.tracks.is_empty() {
                self.current = Some(0);
            }
            self.start_current();
            return self.status.clone();
        }

        if let Some(sink) = &self.sink {
            if self.playing {
                sink.pause();
                self.paused_at = self.position();
                self.started_at = None;
                self.playing = false;
                self.status = "paused".to_string();
            } else {
                sink.play();
                self.started_at = Some(Instant::now() - self.paused_at);
                self.playing = true;
                self.status = format!("playing {}", self.current_title());
            }
        }
        self.dirty = true;
        self.status.clone()
    }

    fn next_track(&mut self) -> String {
        if self.tracks.is_empty() {
            self.status = "no tracks".to_string();
            self.dirty = true;
            return self.status.clone();
        }
        let next = self
            .current
            .map(|idx| (idx + 1) % self.tracks.len())
            .unwrap_or(0);
        self.play_track(next)
    }

    fn prev_track(&mut self) -> String {
        if self.tracks.is_empty() {
            self.status = "no tracks".to_string();
            self.dirty = true;
            return self.status.clone();
        }
        let prev = self
            .current
            .map(|idx| {
                if idx == 0 {
                    self.tracks.len() - 1
                } else {
                    idx - 1
                }
            })
            .unwrap_or(0);
        self.play_track(prev)
    }

    fn set_volume(&mut self, value: f32) -> f32 {
        self.volume = value.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
        self.dirty = true;
        self.volume
    }

    fn seek_to(&mut self, position: Duration) -> String {
        let Some(idx) = self.current else {
            self.status = "no track selected".to_string();
            self.dirty = true;
            return self.status.clone();
        };
        let Some(handle) = &self.handle else {
            self.status = "audio device unavailable".to_string();
            self.dirty = true;
            return self.status.clone();
        };

        let duration = self.current_duration();
        let position = if duration > Duration::ZERO {
            position.min(duration)
        } else {
            position
        };
        let was_playing = self.playing;
        if let Some(old_sink) = self.sink.take() {
            old_sink.stop();
        }

        let path = self.tracks[idx].path.clone();
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => {
                self.status = format!("failed to open {}: {err}", path.display());
                self.dirty = true;
                return self.status.clone();
            }
        };
        let decoder = match Decoder::new(BufReader::new(file)) {
            Ok(decoder) => decoder,
            Err(err) => {
                self.status = format!("failed to decode {}: {err}", path.display());
                self.dirty = true;
                return self.status.clone();
            }
        };
        if self.tracks[idx].duration.is_none() {
            self.tracks[idx].duration = decoder
                .total_duration()
                .or_else(|| probe_audio_duration(&path));
        }

        let sink = match Sink::try_new(handle) {
            Ok(sink) => sink,
            Err(err) => {
                self.status = format!("failed to create sink: {err}");
                self.dirty = true;
                return self.status.clone();
            }
        };
        sink.set_volume(self.volume);
        sink.append(decoder.skip_duration(position));
        if was_playing {
            sink.play();
            self.started_at = Some(Instant::now() - position);
            self.playing = true;
            self.status = format!("playing {}", self.current_title());
        } else {
            sink.pause();
            self.started_at = None;
            self.playing = false;
            self.status = "paused".to_string();
        }
        self.sink = Some(sink);
        self.paused_at = position;
        self.dirty = true;
        self.status.clone()
    }

    fn start_current(&mut self) {
        let Some(idx) = self.current else {
            self.status = "no track selected".to_string();
            self.dirty = true;
            return;
        };

        if let Some(old_sink) = self.sink.take() {
            old_sink.stop();
        }

        let Some(handle) = &self.handle else {
            self.status = "audio device unavailable".to_string();
            self.dirty = true;
            return;
        };

        let path = self.tracks[idx].path.clone();
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => {
                self.status = format!("failed to open {}: {err}", path.display());
                self.dirty = true;
                return;
            }
        };
        let decoder = match Decoder::new(BufReader::new(file)) {
            Ok(decoder) => decoder,
            Err(err) => {
                self.status = format!("failed to decode {}: {err}", path.display());
                self.dirty = true;
                return;
            }
        };
        let duration = decoder
            .total_duration()
            .or_else(|| probe_audio_duration(&path));
        self.tracks[idx].duration = duration;

        let sink = match Sink::try_new(handle) {
            Ok(sink) => sink,
            Err(err) => {
                self.status = format!("failed to create sink: {err}");
                self.dirty = true;
                return;
            }
        };
        sink.set_volume(self.volume);
        sink.append(decoder);
        sink.play();

        self.sink = Some(sink);
        self.started_at = Some(Instant::now());
        self.paused_at = Duration::ZERO;
        self.playing = true;
        self.status = format!("playing {}", self.current_title());
        self.dirty = true;
    }

    fn poll_end_of_track(&mut self) {
        if self.playing && self.sink.as_ref().is_some_and(Sink::empty) {
            self.playing = false;
            self.sink = None;
            self.started_at = None;
            self.paused_at = Duration::ZERO;
            if !self.tracks.is_empty() {
                let _ = self.next_track();
            } else {
                self.dirty = true;
            }
        }
    }

    fn position(&self) -> Duration {
        if self.playing {
            self.started_at
                .map(|started| started.elapsed())
                .unwrap_or(self.paused_at)
        } else {
            self.paused_at
        }
    }

    fn current_duration(&self) -> Duration {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .and_then(|track| track.duration)
            .unwrap_or(Duration::ZERO)
    }

    fn current_title(&self) -> String {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .map(|track| track.title.clone())
            .unwrap_or_else(|| "No track selected".to_string())
    }

    fn current_path(&self) -> String {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .map(|track| track.path.display().to_string())
            .unwrap_or_default()
    }

    fn current_cover_path(&self) -> String {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .and_then(|track| track.cover_path.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    fn current_album_path(&self) -> String {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .map(track_album_path)
            .unwrap_or_default()
    }

    fn current_album_label(&self) -> String {
        self.current
            .and_then(|idx| self.tracks.get(idx))
            .map(track_album_label)
            .unwrap_or_else(|| "Album".to_string())
    }

    fn tracks_value(&self) -> Value {
        Value::List(
            self.tracks
                .iter()
                .enumerate()
                .map(|(idx, track)| {
                    let mut map = HashMap::new();
                    map.insert("index".to_string(), shared(Value::Number(idx as f64)));
                    map.insert(
                        "title".to_string(),
                        shared(Value::String(track.title.clone())),
                    );
                    map.insert(
                        "path".to_string(),
                        shared(Value::String(track.path.display().to_string())),
                    );
                    map.insert(
                        "cover_path".to_string(),
                        shared(Value::String(
                            track
                                .cover_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_default(),
                        )),
                    );
                    map.insert(
                        "duration".to_string(),
                        shared(Value::Number(
                            track.duration.unwrap_or(Duration::ZERO).as_secs_f64(),
                        )),
                    );
                    shared(Value::Map(map))
                })
                .collect(),
        )
    }

    fn albums_value(&self) -> Value {
        Value::List(
            self.album_groups()
                .into_iter()
                .map(|album| {
                    let mut map = HashMap::new();
                    map.insert("label".to_string(), shared(Value::String(album.label)));
                    map.insert("path".to_string(), shared(Value::String(album.path)));
                    map.insert(
                        "cover_path".to_string(),
                        shared(Value::String(
                            album
                                .cover_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_default(),
                        )),
                    );
                    map.insert(
                        "index".to_string(),
                        shared(Value::Number(
                            album
                                .tracks
                                .first()
                                .map(|(idx, _)| *idx as f64)
                                .unwrap_or(0.0),
                        )),
                    );
                    map.insert(
                        "track_count".to_string(),
                        shared(Value::Number(album.tracks.len() as f64)),
                    );
                    shared(Value::Map(map))
                })
                .collect(),
        )
    }

    fn current_album_tracks_value(&self) -> Value {
        let current_album_path = self.current_album_path();
        let Some(album) = self
            .album_groups()
            .into_iter()
            .find(|album| album.path == current_album_path)
        else {
            return Value::List(Vec::new());
        };

        Value::List(
            album
                .tracks
                .into_iter()
                .map(|(idx, track)| track_tree_item(idx, track))
                .collect(),
        )
    }

    fn library_tree_value(&self) -> Value {
        Value::List(
            self.album_groups()
                .into_iter()
                .map(|album| {
                    let children = album
                        .tracks
                        .into_iter()
                        .map(|(idx, track)| track_tree_item(idx, track))
                        .collect();
                    let mut map = HashMap::new();
                    map.insert("label".to_string(), shared(Value::String(album.label)));
                    map.insert("children".to_string(), shared(Value::List(children)));
                    shared(Value::Map(map))
                })
                .collect(),
        )
    }

    fn album_groups(&self) -> Vec<AlbumGroup<'_>> {
        let mut groups: Vec<AlbumGroup<'_>> = Vec::new();
        for (idx, track) in self.tracks.iter().enumerate() {
            let path = track_album_path(track);
            if let Some(album) = groups.iter_mut().find(|album| album.path == path) {
                album.tracks.push((idx, track));
                if album.cover_path.is_none() {
                    album.cover_path = track.cover_path.clone();
                }
            } else {
                groups.push(AlbumGroup {
                    label: track_album_label(track),
                    path,
                    cover_path: track.cover_path.clone(),
                    tracks: vec![(idx, track)],
                });
            }
        }
        for album in &mut groups {
            album.tracks.sort_by(|a, b| a.1.title.cmp(&b.1.title));
        }
        groups.sort_by(|a, b| a.label.cmp(&b.label));
        groups
    }
}

fn track_album_label(track: &Track) -> String {
    track
        .path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Music".to_string())
}

fn track_album_path(track: &Track) -> String {
    track
        .path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default()
}

fn track_tree_item(idx: usize, track: &Track) -> SharedValue {
    let mut map = HashMap::new();
    map.insert("index".to_string(), shared(Value::Number(idx as f64)));
    map.insert(
        "label".to_string(),
        shared(Value::String(track.title.clone())),
    );
    map.insert(
        "path".to_string(),
        shared(Value::String(track.path.display().to_string())),
    );
    map.insert(
        "cover_path".to_string(),
        shared(Value::String(
            track
                .cover_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )),
    );
    map.insert(
        "duration".to_string(),
        shared(Value::Number(
            track.duration.unwrap_or(Duration::ZERO).as_secs_f64(),
        )),
    );
    shared(Value::Map(map))
}

fn scan_music_files() -> Vec<Track> {
    let mut paths = Vec::new();
    collect_audio_files(Path::new("Music"), &mut paths, 0);
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| Track {
            duration: probe_audio_duration(&path),
            title: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            cover_path: find_cover_art(&path),
            path,
        })
        .collect()
}

fn probe_audio_duration(path: &Path) -> Option<Duration> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp3") => mp3_duration::from_path(path).ok(),
        Some("wav") => wav_duration(path),
        _ => None,
    }
}

fn wav_duration(path: &Path) -> Option<Duration> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return None;
    }
    let samples = reader.duration() as f64;
    let seconds = samples / spec.sample_rate as f64;
    Some(Duration::from_secs_f64(seconds))
}

fn find_cover_art(track_path: &Path) -> Option<PathBuf> {
    let folder = track_path.parent()?;
    for name in [
        "cover.png",
        "Cover.png",
        "cover.jpg",
        "Cover.jpg",
        "cover.jpeg",
        "Cover.jpeg",
        "folder.png",
        "Folder.png",
        "folder.jpg",
        "Folder.jpg",
        "folder.jpeg",
        "Folder.jpeg",
        "front.png",
        "Front.png",
        "front.jpg",
        "Front.jpg",
        "front.jpeg",
        "Front.jpeg",
    ] {
        let candidate = folder.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut first_image = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(is_cover_image_extension)
        })
        .collect::<Vec<_>>();
    first_image.sort();
    first_image.into_iter().next()
}

fn is_cover_image_extension(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
}

fn collect_audio_files(root: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 || !root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_audio_files(&path, out, depth + 1);
        } else if is_audio_file(&path) {
            out.push(path);
        }
    }
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp3" | "wav" | "flac" | "ogg"
            )
        })
        .unwrap_or(false)
}

fn usize_arg(args: &[Value], index: usize, message: &str) -> Result<usize, String> {
    let Some(Value::Number(value)) = args.get(index) else {
        return Err(message.to_string());
    };
    Ok((*value).max(0.0).round() as usize)
}

fn number_arg(args: &[Value], index: usize, message: &str) -> Result<f64, String> {
    let Some(Value::Number(value)) = args.get(index) else {
        return Err(message.to_string());
    };
    Ok(*value)
}

fn current_index_value(current: Option<usize>) -> Value {
    current
        .map(|idx| Value::Number(idx as f64))
        .unwrap_or(Value::Nil)
}

fn shared(value: Value) -> SharedValue {
    Rc::new(RefCell::new(value))
}
