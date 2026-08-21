/*!
Builds the scratch-buffer control runtimes and loads the bundled lisp
libraries.

`scratch_runtime_with_fallbacks` (UI side) and
`scheduler_scratch_runtime_with_fallbacks` construct a
`ScratchControlRuntime` (struct in `shared_state`) with the control prelude,
the scratch-buffer template shown on first open, and fallback descriptors.
Also owns discovery/loading of the two on-disk lisp libraries: the MIDI-FX
library (`load_midi_fx_library_source`, `load_midi_fx_descriptors`, path
resolution under the midi-fx library roots) and the process library
(`load_process_library_source`, which appends the compiled-in
`processes/builtin.lisp`), plus the metadata parsing that turns library
sources into `EffectDescriptor`s for the browser.
*/

use super::super::*;

pub(in crate::lisp_host) fn scratch_buffer_template() -> String {
    r#"; Scratch buffer for live sequencer scripting.
; C-x C-e eval s-expression at cursor
; C-x C-b eval whole buffer
; C-q quit scratch
; Examples:
;   (seq-track-steps)
;   (for-each |n| (seq-toggle-step n) (list 1 5 9 13))
;   (every :bar 2 '(seq-toggle-step 0))
;   (clear-hooks)

(seq-track-steps)
"#
    .to_string()
}

pub(in crate::lisp_host) fn control_prelude_source() -> &'static str {
    r#"
(def empty? (xs) (= (len xs) 0))
(def map (fn xs)
  (if (empty? xs)
    '()
    (cons (fn (first xs))
          (map fn (rest xs)))))
(def filter (fn xs)
  (if (empty? xs)
    '()
    (if (fn (first xs))
      (cons (first xs) (filter fn (rest xs)))
      (filter fn (rest xs)))))
(def reduce (fn acc xs)
  (if (empty? xs)
    acc
    (reduce fn (fn acc (first xs)) (rest xs))))
(def for-each (fn xs)
  (if (empty? xs)
    nil
    (do
      (fn (first xs))
      (for-each fn (rest xs)))))
"#
}

pub(in crate::lisp_host) fn new_eval_context(track: usize, cursor_step: usize) -> SharedSequencerEvalContext {
    Arc::new(Mutex::new(SequencerEvalContext { track, cursor_step }))
}



pub fn scratch_runtime_with_fallbacks(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
) -> ScratchControlRuntime {
    scratch_runtime_with_fallbacks_inner(state, track, cursor_step, true)
}

pub fn scheduler_scratch_runtime_with_fallbacks(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
) -> ScratchControlRuntime {
    scratch_runtime_with_fallbacks_inner(state, track, cursor_step, false)
}

pub(in crate::lisp_host) fn scratch_runtime_with_fallbacks_inner(
    state: Arc<crate::sequencer::SequencerState>,
    track: usize,
    cursor_step: usize,
    write_process_chain_state: bool,
) -> ScratchControlRuntime {
    let track_count = state.active_track_count().max(1);
    let (effect_descriptors, instrument_descriptors) = state.scratch_runtime_descriptors();
    let effect_descriptors = if effect_descriptors.is_empty() {
        fallback_effect_descriptors(track_count)
    } else {
        effect_descriptors
    };
    let instrument_descriptors = if instrument_descriptors.is_empty() {
        fallback_instrument_descriptors(track_count)
    } else {
        instrument_descriptors
    };
    let mut runtime = ScratchControlRuntime::new_with_process_chain_writes(
        state,
        effect_descriptors,
        instrument_descriptors,
        track,
        cursor_step,
        write_process_chain_state,
    );
    runtime.set_theme_sync_enabled(false);
    runtime
}

pub(in crate::lisp_host) fn midi_fx_library_root_candidates() -> Vec<PathBuf> {
    let root = crate::app_paths::app_paths().midi_fx_dir();
    if root.is_dir() {
        vec![root.canonicalize().unwrap_or(root)]
    } else {
        Vec::new()
    }
}

pub(in crate::lisp_host) fn midi_fx_name_components(name: &str) -> Option<Vec<&str>> {
    let trimmed = name.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut components = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(value) => components.push(value.to_str()?),
            _ => return None,
        }
    }
    if components.is_empty() {
        None
    } else {
        Some(components)
    }
}

pub(in crate::lisp_host) fn midi_fx_source_path(name: &str) -> Option<PathBuf> {
    let components = midi_fx_name_components(name)?;
    for root in midi_fx_library_root_candidates() {
        let mut folder = root.clone();
        for component in &components {
            folder.push(component);
        }
        let folder_dsp = folder.join("dsp.lisp");
        if folder_dsp.exists() {
            return Some(folder_dsp);
        }

        let mut file = root;
        for component in &components[..components.len().saturating_sub(1)] {
            file.push(component);
        }
        file.push(format!("{}.lisp", components[components.len() - 1]));
        if file.exists() {
            return Some(file);
        }
    }
    None
}

pub(in crate::lisp_host) fn read_midi_fx_lisp(path: &Path) -> io::Result<String> {
    let source = std::fs::read_to_string(path)?;
    eseqlisp::module_alias_migration::warn_on_old_module_aliases(path, &source);
    Ok(source)
}

pub(in crate::lisp_host) fn load_midi_fx_source(name: &str) -> io::Result<String> {
    let path = midi_fx_source_path(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "MIDI FX source not found"))?;
    read_midi_fx_lisp(&path)
}

pub fn load_midi_fx_library_source() -> String {
    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                let dsp = path.join("dsp.lisp");
                if dsp.exists() {
                    if let (Ok(rel), Ok(src)) =
                        (path.strip_prefix(root), read_midi_fx_lisp(&dsp))
                    {
                        out.push((rel.to_string_lossy().replace('\\', "/"), src));
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let mut sources = Vec::new();
    for root in midi_fx_library_root_candidates() {
        collect(&root, &root, &mut sources);
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
        .into_iter()
        .map(|(name, src)| format!("; midi-fx/{name}/dsp.lisp\n{src}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn load_process_library_source() -> String {
    let path = crate::app_paths::app_paths().processes_dir().join("builtin.lisp");
    match std::fs::read_to_string(&path) {
        Ok(source) if source.trim().is_empty() => String::new(),
        Ok(_) => {
            // Evaluate through `load` so def-process can retain the VM's real
            // source module for slot-to-source navigation. In packaged builds
            // where the source tree is unavailable, retain the embedded
            // definitions as a functional fallback.
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            format!("(load {:?})", path.to_string_lossy())
        }
        Err(_) => {
            let source = include_str!("../../../../../content/processes/builtin.lisp");
            if source.trim().is_empty() {
                String::new()
            } else {
                format!("; embedded processes/builtin.lisp\n{source}\n")
            }
        }
    }
}

pub(in crate::lisp_host) fn load_midi_fx_descriptors_from_source(source: String) -> Vec<EffectDescriptor> {
    if source.trim().is_empty() {
        return Vec::new();
    }

    let cache = MIDI_FX_DESCRIPTOR_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().expect("midi fx descriptor cache poisoned");
        if let Some(cached) = guard.get(&source) {
            return cached.clone();
        }
    }

    if let Ok(descriptors) = parse_midi_fx_descriptors_from_source(&source) {
        if !descriptors.is_empty() {
            cache
                .lock()
                .expect("midi fx descriptor cache poisoned")
                .insert(source, descriptors.clone());
            return descriptors;
        }
    }

    let state = Arc::new(crate::sequencer::SequencerState::new(
        1,
        vec![crate::sequencer::default_empty_effect_chain()],
    ));
    let mut runtime = ScratchControlRuntime::new(
        Arc::clone(&state),
        fallback_effect_descriptors(1),
        fallback_instrument_descriptors(1),
        0,
        0,
    );
    let descriptors = if runtime.eval(&source).is_err() {
        Vec::new()
    } else {
        runtime.midi_fx_descriptors()
    };
    cache
        .lock()
        .expect("midi fx descriptor cache poisoned")
        .insert(source, descriptors.clone());
    descriptors
}

pub(in crate::lisp_host) fn parse_midi_fx_descriptors_from_source(source: &str) -> Result<Vec<EffectDescriptor>, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize MIDI FX source: {error:?}"))?;
    let expressions = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse MIDI FX source: {error:?}"))?;
    let mut pending_params = Vec::new();
    let mut descriptors: Vec<EffectDescriptor> = Vec::new();

    for expression in expressions {
        let Expression::List(items) = expression else {
            continue;
        };
        let Some(Expression::Symbol(operator)) = items.first() else {
            continue;
        };
        match operator.as_str() {
            "midi-fx-param" => {
                let Some(name) = items.get(1).and_then(midi_fx_metadata_name) else {
                    return Err("midi-fx-param expects a name".to_string());
                };
                let args = items[2..]
                    .iter()
                    .map(midi_fx_metadata_value)
                    .collect::<Result<Vec<_>, _>>()?;
                pending_params.push(parse_midi_fx_param_descriptor(&name, &args)?);
            }
            "def-midi-fx" => {
                let Some(name) = items.get(1).and_then(midi_fx_metadata_name) else {
                    return Err("def-midi-fx expects a name".to_string());
                };
                let mut params = std::mem::take(&mut pending_params);
                ensure_enabled_param(&mut params);
                for (idx, param) in params.iter_mut().enumerate() {
                    param.node_param_idx = idx as u32;
                }
                let mut descriptor = EffectDescriptor::empty_custom_slot();
                descriptor.name = name.clone();
                descriptor.params = params;
                if let Some(existing) = descriptors.iter_mut().find(|desc| desc.name == name) {
                    *existing = descriptor;
                } else {
                    descriptors.push(descriptor);
                }
            }
            _ => {}
        }
    }

    Ok(descriptors)
}

pub(in crate::lisp_host) fn midi_fx_metadata_name(expression: &Expression) -> Option<String> {
    match expression {
        Expression::String(name) | Expression::Symbol(name) | Expression::Keyword(name) => {
            Some(name.trim_start_matches('@').to_string())
        }
        _ => None,
    }
}

pub(in crate::lisp_host) fn midi_fx_metadata_value(expression: &Expression) -> Result<EValue, String> {
    match expression {
        Expression::String(value) => Ok(EValue::String(value.clone())),
        Expression::Symbol(value) => Ok(EValue::Symbol(value.clone())),
        Expression::Keyword(value) => Ok(EValue::Keyword(value.clone())),
        Expression::Number(value) => Ok(EValue::Number(*value)),
        _ => Err("MIDI FX metadata supports only literal parameter attributes".to_string()),
    }
}

pub fn midi_fx_library_source_with_user_source(user_source: &str) -> String {
    let library = load_midi_fx_library_source();
    if library.trim().is_empty() {
        user_source.to_string()
    } else if user_source.trim().is_empty() {
        library
    } else {
        format!("{library}\n; *scratch*\n{user_source}")
    }
}

pub fn process_library_source_with_user_source(user_source: &str) -> String {
    let library = load_process_library_source();
    if library.trim().is_empty() {
        user_source.to_string()
    } else if user_source.trim().is_empty() {
        library
    } else {
        format!("{library}\n; *scratch*\n{user_source}")
    }
}

pub fn load_midi_fx_descriptors() -> Vec<EffectDescriptor> {
    load_midi_fx_descriptors_from_source(load_midi_fx_library_source())
}

pub fn load_midi_fx_descriptor(name: &str) -> Option<EffectDescriptor> {
    if let Ok(source) = load_midi_fx_source(name) {
        if let Some(descriptor) = load_midi_fx_descriptors_from_source(source)
            .into_iter()
            .find(|desc| desc.name.eq_ignore_ascii_case(name))
        {
            return Some(descriptor);
        }
    }

    load_midi_fx_descriptors()
        .into_iter()
        .find(|desc| desc.name.eq_ignore_ascii_case(name))
}

pub fn eval_sequencer_control(
    code: &str,
    state: Arc<crate::sequencer::SequencerState>,
    track: Option<usize>,
    cursor_step: Option<usize>,
) -> Result<Option<EValue>, String> {
    let mut runtime = Runtime::new();
    let track_count = state.active_track_count();
    register_sequencer_natives(
        &mut runtime,
        state,
        new_eval_context(track.unwrap_or(0), cursor_step.unwrap_or(0)),
        shared_native_metadata(
            fallback_effect_descriptors(track_count),
            fallback_instrument_descriptors(track_count),
        ),
    );
    runtime
        .eval_str(control_prelude_source())
        .map_err(|e| format!("{e:?}"))?;
    runtime.eval_str(code).map_err(|e| format!("{e:?}"))
}



/// Run the instrument edit → compile → name → save flow.
/// Called while terminal is in normal (non-raw) mode.
/// Does NOT wire nodes — the caller handles graph wiring.
pub fn run_instrument_editor_flow(
    last_source: &str,
    existing_name: Option<&str>,
    sample_rate: u32,
) -> Option<InstrumentEditResult> {
    let initial = if last_source.is_empty() {
        INSTRUMENT_TEMPLATE.to_string()
    } else {
        last_source.to_string()
    };

    let mut source = initial;

    loop {
        match edit_text(&source) {
            Ok(edited) => {
                source = edited;
            }
            Err(e) => {
                eprintln!("Editor error: {e}");
                return None;
            }
        }

        print!("Compiling instrument...");
        io::stdout().flush().ok();

        match compile_instrument(&source, sample_rate) {
            Ok(json) => match parse_manifest(&json) {
                Ok(manifest) => match load_dylib_prewarmed(&manifest) {
                    Ok(lib) => {
                        println!(" OK!");
                        let n = manifest.params.len();
                        if n > 0 {
                            println!("  Parameters:");
                            for p in &manifest.params {
                                println!(
                                    "    {} = {} [{}, {}]{}",
                                    p.name,
                                    p.default,
                                    p.min,
                                    p.max,
                                    p.unit
                                        .as_deref()
                                        .map(|u| format!(" {u}"))
                                        .unwrap_or_default()
                                );
                            }
                        }

                        let default_name = existing_name.unwrap_or("");
                        if default_name.is_empty() {
                            print!("\nInstrument name: ");
                        } else {
                            print!("\nInstrument name [{}]: ", default_name);
                        }
                        io::stdout().flush().ok();
                        let mut name_buf = String::new();
                        std::io::stdin().read_line(&mut name_buf).ok();
                        let name_input = name_buf.trim();
                        let name = if name_input.is_empty() {
                            if default_name.is_empty() {
                                "untitled".to_string()
                            } else {
                                default_name.to_string()
                            }
                        } else {
                            sanitize_effect_name(name_input)
                        };

                        match save_instrument(&name, &source) {
                            Ok(()) => println!("Saved to instruments/{}.lisp", name),
                            Err(e) => eprintln!("Warning: failed to save: {e}"),
                        }

                        println!("\nInstrument '{}' compiled successfully.", name);
                        let params = manifest.params.clone();
                        return Some(InstrumentEditResult {
                            manifest,
                            lib,
                            lease: None,
                            source,
                            params,
                            name,
                        });
                    }
                    Err(e) => eprintln!(" Failed to load dylib: {e}"),
                },
                Err(e) => eprintln!(" Failed to parse manifest: {e}"),
            },
            Err(e) => {
                println!();
                eprintln!("Compile error:\n{e}");
            }
        }

        eprint!("\nPress Enter to re-edit, or 'q' + Enter to cancel: ");
        io::stdout().flush().ok();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
        if buf.trim() == "q" {
            return None;
        }
    }
}
