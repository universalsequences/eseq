use std::path::Path;

use eseqlisp::Editor;

pub(crate) fn lisp_string_literal(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn expr_to_lisp(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::Symbol(s) => s.clone(),
        Expression::Keyword(s) => format!(":{s}"),
        Expression::String(s) => lisp_string_literal(s),
        Expression::QuoteSymbol(s) => format!("'{s}"),
        Expression::QuoteList(items) => format!(
            "'({})",
            items.iter().map(expr_to_lisp).collect::<Vec<_>>().join(" ")
        ),
        Expression::Number(n) => {
            if n.fract() == 0.0 {
                format!("{n:.0}")
            } else {
                n.to_string()
            }
        }
        Expression::List(items) => format!(
            "({})",
            items.iter().map(expr_to_lisp).collect::<Vec<_>>().join(" ")
        ),
        Expression::Quasiquote(inner) => format!("`{}", expr_to_lisp(inner)),
        Expression::Unquote(inner) => format!(",{}", expr_to_lisp(inner)),
    }
}

fn custom_ui_param_name(expr: &eseqlisp::parser::Expression) -> Option<String> {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::String(name) | Expression::Symbol(name) => Some(name.clone()),
        Expression::List(items) => items.first().and_then(custom_ui_param_name),
        _ => None,
    }
}

fn is_fill_expr(expr: &eseqlisp::parser::Expression) -> bool {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::Keyword(value) | Expression::Symbol(value) => value == "fill",
        _ => false,
    }
}

fn transform_layout_items_without_unbounded_width(
    items: &[eseqlisp::parser::Expression],
    transform: fn(&eseqlisp::parser::Expression) -> String,
) -> Vec<String> {
    use eseqlisp::parser::Expression;

    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if matches!(&items[i], Expression::Keyword(key) if key == "width")
            && items.get(i + 1).is_some_and(is_fill_expr)
        {
            i += 2;
            continue;
        }
        out.push(transform(&items[i]));
        i += 1;
    }
    out
}

fn transform_synth_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if head == "tabs" {
                    let mut out = Vec::new();
                    let mut i = 0;
                    let mut has_header_height = false;
                    while i < items.len() {
                        if matches!(&items[i], Expression::Keyword(k) if k == "header-height") {
                            has_header_height = true;
                            out.push(":header-height".to_string());
                            out.push("1.0".to_string());
                            i += 2;
                            continue;
                        }
                        out.push(transform_synth_ui_expr(&items[i]));
                        i += 1;
                    }
                    if !has_header_height {
                        out.insert(1, ":header-height".to_string());
                        out.insert(2, "1.0".to_string());
                    }
                    return format!("({})", out.join(" "));
                }
                if head == "param" {
                    if let Some(name) = items.get(1).and_then(custom_ui_param_name) {
                        return format!("(ui-param-control {})", lisp_string_literal(&name));
                    }
                }
                if head == "params" {
                    let mut controls = Vec::new();
                    for item in items.iter().skip(1) {
                        if matches!(item, Expression::Keyword(_)) {
                            continue;
                        }
                        if let Some(name) = custom_ui_param_name(item) {
                            controls
                                .push(format!("(ui-param-control {})", lisp_string_literal(&name)));
                        }
                    }
                    return format!("(v-stack :gap 0.25 {})", controls.join(" "));
                }
            }
            format!(
                "({})",
                transform_layout_items_without_unbounded_width(items, transform_synth_ui_expr)
                    .join(" ")
            )
        }
        _ => expr_to_lisp(expr),
    }
}

fn transform_midi_fx_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if head == "midi-fx-param" {
                    if let Some(name) = items.get(1).and_then(custom_ui_param_name) {
                        return format!(
                            "(midi-fx-ui-param-control {})",
                            lisp_string_literal(&name)
                        );
                    }
                }
                if head == "params" {
                    let mut controls = Vec::new();
                    for item in items.iter().skip(1) {
                        if matches!(item, Expression::Keyword(_)) {
                            continue;
                        }
                        if let Some(name) = custom_ui_param_name(item) {
                            controls.push(format!(
                                "(midi-fx-ui-param-control {})",
                                lisp_string_literal(&name)
                            ));
                        }
                    }
                    return format!("(v-stack :gap 0.25 {})", controls.join(" "));
                }
            }
            format!(
                "({})",
                transform_layout_items_without_unbounded_width(items, transform_midi_fx_ui_expr)
                    .join(" ")
            )
        }
        _ => expr_to_lisp(expr),
    }
}

fn transform_audio_fx_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if matches!(head.as_str(), "effect-param" | "fx-param" | "param") {
                    if let Some(name) = items.get(1).and_then(custom_ui_param_name) {
                        return format!(
                            "(audio-fx-ui-param-control {})",
                            lisp_string_literal(&name)
                        );
                    }
                }
                if head == "params" {
                    let mut controls = Vec::new();
                    for item in items.iter().skip(1) {
                        if matches!(item, Expression::Keyword(_)) {
                            continue;
                        }
                        if let Some(name) = custom_ui_param_name(item) {
                            controls.push(format!(
                                "(audio-fx-ui-param-control {})",
                                lisp_string_literal(&name)
                            ));
                        }
                    }
                    return format!("(v-stack :gap 0.25 {})", controls.join(" "));
                }
            }
            format!(
                "({})",
                transform_layout_items_without_unbounded_width(items, transform_audio_fx_ui_expr)
                    .join(" ")
            )
        }
        _ => expr_to_lisp(expr),
    }
}

fn safe_lisp_ident(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub(crate) fn build_custom_instrument_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Expression, Parser};

    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String, String)>) {
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
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            let inst_name =
                                format!("{}/", rel.to_string_lossy().replace('\\', "/"));
                            out.push((inst_name, ui_path.display().to_string(), src));
                        }
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let root = Path::new("instruments");
    let mut ui_sources = Vec::new();
    collect(root, root, &mut ui_sources);

    let mut functions = String::new();
    let mut dispatch = "false".to_string();
    if let Some((instrument_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources
            .iter_mut()
            .find(|(name, _, _)| name == &instrument_name)
        {
            *existing = (instrument_name, ui_path, src);
        } else {
            ui_sources.push((instrument_name, ui_path, src));
        }
    }

    for (instrument_name, ui_path, src) in ui_sources {
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom instrument UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom instrument UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        let mut body = None;
        let mut helpers = Vec::new();
        for expr in &exprs {
            if let Expression::List(items) = expr {
                if matches!(items.first(), Some(Expression::Symbol(head)) if head == "defsynth-ui")
                {
                    body = items.get(1).map(transform_synth_ui_expr);
                    continue;
                }
            }
            helpers.push(transform_synth_ui_expr(expr));
        }
        let Some(body) = body else {
            eprintln!(
                "[custom-ui] instrument ui skipped name={instrument_name:?} path={ui_path}: missing defsynth-ui"
            );
            continue;
        };
        let normalized_instrument_name = instrument_name.trim_end_matches('/');
        let fn_name = format!("custom-synth-ui-{}", safe_lisp_ident(&instrument_name));
        for helper in helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&format!(
            "\n(def {fn_name} (inst) (do (set! synth-ui-current-inst inst) (set! synth-ui-current-name {}) (set! custom-ui-current-kind \"instrument\") (set! custom-ui-selected-section (custom-ui-selected-section-for-current-scope)) {body}))\n",
            lisp_string_literal(normalized_instrument_name)
        ));
        let name_match = if normalized_instrument_name == instrument_name {
            format!(
                "(= (get inst :name) {})",
                lisp_string_literal(&instrument_name)
            )
        } else {
            format!(
                "(or (= (get inst :name) {}) (= (get inst :name) {}))",
                lisp_string_literal(&instrument_name),
                lisp_string_literal(normalized_instrument_name)
            )
        };
        dispatch = format!("(if {name_match} ({fn_name} inst) {dispatch})");
    }

    format!("{functions}\n(def custom-instrument-synth-ui (inst) {dispatch})\n")
}

pub(crate) fn build_custom_midi_fx_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Expression, Parser};

    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String, String)>) {
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
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            let fx_name = rel.to_string_lossy().replace('\\', "/");
                            out.push((fx_name, ui_path.display().to_string(), src));
                        }
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let root = Path::new("midi-fx");
    let mut ui_sources = Vec::new();
    collect(root, root, &mut ui_sources);

    if let Some((fx_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources.iter_mut().find(|(name, _, _)| name == &fx_name) {
            *existing = (fx_name, ui_path, src);
        } else {
            ui_sources.push((fx_name, ui_path, src));
        }
    }

    let mut functions = String::new();
    let mut dispatch = "false".to_string();
    for (fx_name, ui_path, src) in ui_sources {
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom MIDI FX UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom MIDI FX UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        let mut body = None;
        let mut helpers = Vec::new();
        for expr in &exprs {
            if let Expression::List(items) = expr {
                if matches!(items.first(), Some(Expression::Symbol(head)) if head == "def-midi-fx-ui")
                {
                    body = items.get(1).map(transform_midi_fx_ui_expr);
                    continue;
                }
            }
            helpers.push(transform_midi_fx_ui_expr(expr));
        }
        let Some(body) = body else {
            continue;
        };
        let fn_name = format!("custom-midi-fx-ui-{}", safe_lisp_ident(&fx_name));
        for helper in helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&format!(
            "\n(def {fn_name} (fx) (do (set! midi-fx-ui-current-fx fx) (set! midi-fx-ui-current-name {}) {body}))\n",
            lisp_string_literal(&fx_name)
        ));
        dispatch = format!(
            "(if (= (get fx :name) {}) ({fn_name} fx) {dispatch})",
            lisp_string_literal(&fx_name)
        );
    }

    format!("{functions}\n(def custom-midi-fx-ui (fx) {dispatch})\n")
}

pub(crate) fn build_custom_audio_fx_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String {
    use eseqlisp::parser::{ASTParser, Expression, Parser};

    fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String, String)>) {
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
                if path.join("dsp.lisp").exists() {
                    let ui_path = path.join("ui.lisp");
                    if ui_path.exists() {
                        if let (Ok(rel), Ok(src)) =
                            (path.strip_prefix(root), std::fs::read_to_string(&ui_path))
                        {
                            let fx_name = rel.to_string_lossy().replace('\\', "/");
                            out.push((fx_name, ui_path.display().to_string(), src));
                        }
                    }
                }
                collect(&path, root, out);
            }
        }
    }

    let root = Path::new("effects");
    let mut ui_sources = Vec::new();
    collect(root, root, &mut ui_sources);

    if let Some((fx_name, ui_path, src)) = overlay {
        if let Some(existing) = ui_sources.iter_mut().find(|(name, _, _)| name == &fx_name) {
            *existing = (fx_name, ui_path, src);
        } else {
            ui_sources.push((fx_name, ui_path, src));
        }
    }

    let mut functions = String::new();
    let mut dispatch = "false".to_string();
    for (fx_name, ui_path, src) in ui_sources {
        let tokens = match Parser::new(src).parse() {
            Ok(tokens) => tokens,
            Err(err) => {
                eprintln!("custom audio effect UI parse error in {ui_path}: {err:?}");
                continue;
            }
        };
        let exprs = match ASTParser::new(tokens).parse() {
            Ok(exprs) => exprs,
            Err(err) => {
                eprintln!("custom audio effect UI AST error in {ui_path}: {err:?}");
                continue;
            }
        };
        let mut body = None;
        let mut helpers = Vec::new();
        for expr in &exprs {
            if let Expression::List(items) = expr {
                if matches!(items.first(), Some(Expression::Symbol(head)) if head == "defeffect-ui")
                {
                    body = items.get(1).map(transform_audio_fx_ui_expr);
                    continue;
                }
            }
            helpers.push(transform_audio_fx_ui_expr(expr));
        }
        let Some(body) = body else {
            eprintln!(
                "[custom-ui] audio effect ui skipped name={fx_name:?} path={ui_path}: missing defeffect-ui"
            );
            continue;
        };
        let fn_name = format!("custom-audio-fx-ui-{}", safe_lisp_ident(&fx_name));
        for helper in helpers {
            functions.push_str(&format!("\n{helper}\n"));
        }
        functions.push_str(&format!(
            "\n(def {fn_name} (fx) (do (set! audio-fx-ui-current-fx fx) (set! audio-fx-ui-current-name {}) (set! custom-ui-current-kind \"audio-fx\") (set! custom-ui-selected-section (custom-ui-selected-section-for-current-scope)) {body}))\n",
            lisp_string_literal(&fx_name)
        ));
        dispatch = format!(
            "(if (= (get fx :name) {}) ({fn_name} fx) {dispatch})",
            lisp_string_literal(&fx_name)
        );
    }

    format!("{functions}\n(def custom-audio-fx-ui (fx) {dispatch})\n")
}

pub(crate) fn reload_custom_instrument_ui(editor: &mut Editor) {
    let custom_ui_source =
        build_custom_instrument_ui_source_with_overlay(active_custom_ui_buffer_overlay(editor));
    if !custom_ui_source.is_empty() {
        if let Err(err) = editor.runtime_mut().eval_str(&custom_ui_source) {
            eprintln!("custom instrument UI load error: {err:?}");
        }
    }
    let custom_midi_fx_source = build_custom_midi_fx_ui_source_with_overlay(
        active_custom_midi_fx_ui_buffer_overlay(editor),
    );
    if !custom_midi_fx_source.is_empty() {
        if let Err(err) = editor.runtime_mut().eval_str(&custom_midi_fx_source) {
            eprintln!("custom MIDI FX UI load error: {err:?}");
        }
    }
    let custom_audio_fx_source = build_custom_audio_fx_ui_source_with_overlay(
        active_custom_audio_fx_ui_buffer_overlay(editor),
    );
    if !custom_audio_fx_source.is_empty() {
        if let Err(err) = editor.runtime_mut().eval_str(&custom_audio_fx_source) {
            eprintln!("custom audio effect UI load error: {err:?}");
        }
    }
}

fn active_custom_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let root = Path::new("instruments");
    let rel = folder.strip_prefix(root).ok()?;
    let instrument_name = format!("{}/", rel.to_string_lossy().replace('\\', "/"));
    Some((instrument_name, path.display().to_string(), buffer.text()))
}

fn active_custom_midi_fx_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let root = Path::new("midi-fx");
    let rel = folder.strip_prefix(root).ok()?;
    let fx_name = rel.to_string_lossy().replace('\\', "/");
    Some((fx_name, path.display().to_string(), buffer.text()))
}

fn active_custom_audio_fx_ui_buffer_overlay(editor: &Editor) -> Option<(String, String, String)> {
    let buffer = editor.active_buffer();
    let path = buffer.path.as_ref()?;
    if path.file_name().and_then(|name| name.to_str()) != Some("ui.lisp") {
        return None;
    }
    let folder = path.parent()?;
    if !folder.join("dsp.lisp").exists() {
        return None;
    }
    let root = Path::new("effects");
    let rel = folder.strip_prefix(root).ok()?;
    let fx_name = rel.to_string_lossy().replace('\\', "/");
    Some((fx_name, path.display().to_string(), buffer.text()))
}
