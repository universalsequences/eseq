#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("eseqlisp_capture requires macOS Metal");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    use eseqlisp::backend::Backend;
    use eseqlisp::editor::ViewMode;
    use eseqlisp::frame::build_render_frame;
    use eseqlisp::metal_backend::MetalBackend;
    use eseqlisp::{Editor, EditorConfig, Runtime};

    let args = CaptureArgs::parse(std::env::args().skip(1))?;
    let source = args.source()?;

    let mut backend = MetalBackend::new_capture(args.width, args.height)
        .map_err(|_| "failed to create Metal backend".to_string())?;
    backend
        .initialize()
        .map_err(|_| "failed to initialize Metal backend".to_string())?;

    let runtime = Runtime::new();
    let mut editor = Editor::new(runtime, EditorConfig::default());
    let (cell_w, cell_h) = backend.cell_dimensions();
    if let Some(measurer) = backend.create_text_measurer() {
        editor.set_text_measurer(measurer, cell_w, cell_h);
    }

    editor
        .runtime_mut()
        .eval_str(&source)
        .map_err(|error| format!("lisp evaluation failed: {error:?}"))?;
    if editor.widget_layout().is_some() {
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
    }

    let cols = (args.width as f32 / cell_w).floor().max(1.0) as usize;
    let rows = (args.height as f32 / cell_h).floor().max(1.0) as usize;
    if let Some((delta_x, delta_y)) = args.touchpad_scroll {
        let _ = build_render_frame(&mut editor, cols, rows);
        let precise_col = (cols as f32 * 0.5).max(1.0);
        let precise_row = (rows as f32 * 0.5).max(1.0);
        if !editor.handle_touchpad_scroll(0, 0, precise_col, precise_row, delta_x, delta_y) {
            return Err("synthetic touchpad scroll was not handled by a widget".to_string());
        }
    }
    let frame = build_render_frame(&mut editor, cols, rows);
    backend
        .render_frame_to_png(&frame, args.width, args.height, &args.out)
        .map_err(|_| "failed to render capture PNG".to_string())?;

    println!("{}", args.out.display());
    Ok(())
}

#[cfg(target_os = "macos")]
struct CaptureArgs {
    source: Option<String>,
    source_file: Option<std::path::PathBuf>,
    width: u32,
    height: u32,
    out: std::path::PathBuf,
    touchpad_scroll: Option<(f32, f32)>,
}

#[cfg(target_os = "macos")]
impl CaptureArgs {
    fn parse<I>(mut args: I) -> Result<Self, String>
    where
        I: Iterator<Item = String>,
    {
        let mut parsed = CaptureArgs {
            source: None,
            source_file: None,
            width: 1600,
            height: 1000,
            out: std::path::PathBuf::from("/tmp/eseqlisp-capture.png"),
            touchpad_scroll: None,
        };

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--source" => parsed.source = Some(next_value(&mut args, "--source")?),
                "--source-file" => {
                    parsed.source_file = Some(std::path::PathBuf::from(next_value(
                        &mut args,
                        "--source-file",
                    )?));
                }
                "--width" => parsed.width = parse_u32(&mut args, "--width")?,
                "--height" => parsed.height = parse_u32(&mut args, "--height")?,
                "--touchpad-scroll" => {
                    let delta_x = parse_f32(&mut args, "--touchpad-scroll delta-x")?;
                    let delta_y = parse_f32(&mut args, "--touchpad-scroll delta-y")?;
                    parsed.touchpad_scroll = Some((delta_x, delta_y));
                }
                "--out" => parsed.out = std::path::PathBuf::from(next_value(&mut args, "--out")?),
                "-h" | "--help" => return Err(Self::usage()),
                other => return Err(format!("unknown argument {other}\n{}", Self::usage())),
            }
        }

        if parsed.source.is_some() == parsed.source_file.is_some() {
            return Err(format!(
                "provide exactly one of --source or --source-file\n{}",
                Self::usage()
            ));
        }
        Ok(parsed)
    }

    fn source(&self) -> Result<String, String> {
        if let Some(source) = &self.source {
            return Ok(source.clone());
        }
        let Some(path) = &self.source_file else {
            return Err("missing source".to_string());
        };
        std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
    }

    fn usage() -> String {
        "usage: eseqlisp_capture (--source LISP | --source-file PATH) [--width PX] [--height PX] [--touchpad-scroll DX DY] --out PATH"
            .to_string()
    }
}

#[cfg(target_os = "macos")]
fn next_value<I>(args: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(target_os = "macos")]
fn parse_u32<I>(args: &mut I, flag: &str) -> Result<u32, String>
where
    I: Iterator<Item = String>,
{
    let raw = next_value(args, flag)?;
    let value = raw
        .parse::<u32>()
        .map_err(|_| format!("{flag} expects a positive integer"))?;
    if value == 0 {
        return Err(format!("{flag} expects a positive integer"));
    }
    Ok(value)
}

#[cfg(target_os = "macos")]
fn parse_f32<I>(args: &mut I, flag: &str) -> Result<f32, String>
where
    I: Iterator<Item = String>,
{
    let raw = next_value(args, flag)?;
    raw.parse::<f32>()
        .map_err(|_| format!("{flag} expects a number"))
}
