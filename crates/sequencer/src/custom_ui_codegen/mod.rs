//! The source transform that turns an instrument's `ui.lisp` into the
//! instrument-panel functions the effects UI dispatches to: `(param x)` becomes
//! a runtime control, the file's own helpers are namespaced per instrument,
//! and `defsynth-ui` becomes `custom-synth-ui-<id>`. The app builds one
//! dispatcher over every instrument (`ui::custom_ui`); plugin export builds it
//! for exactly one ([`single_instrument_ui_source`]).

use std::collections::BTreeSet;

pub fn lisp_string_literal(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

pub fn expr_to_lisp(expr: &eseqlisp::parser::Expression) -> String {
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
        Expression::UnquoteSplicing(inner) => format!(",@{}", expr_to_lisp(inner)),
    }
}

pub fn custom_ui_param_name(expr: &eseqlisp::parser::Expression) -> Option<String> {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::String(name) | Expression::Symbol(name) => Some(name.clone()),
        Expression::List(items) => items.first().and_then(custom_ui_param_name),
        _ => None,
    }
}

pub fn is_fill_expr(expr: &eseqlisp::parser::Expression) -> bool {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::Keyword(value) | Expression::Symbol(value) => value == "fill",
        _ => false,
    }
}

pub fn transform_layout_items_without_unbounded_width(
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

pub fn transform_synth_ui_expr(expr: &eseqlisp::parser::Expression) -> String {
    use eseqlisp::parser::Expression;
    match expr {
        Expression::List(items) if !items.is_empty() => {
            if let Expression::Symbol(head) = &items[0] {
                if head == "defwidget" {
                    return expr_to_lisp(expr);
                }
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
                        return format!("(eseq.effects.custom-ui-runtime/ui-param-control {})", lisp_string_literal(&name));
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
                                .push(format!("(eseq.effects.custom-ui-runtime/ui-param-control {})", lisp_string_literal(&name)));
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

pub fn safe_lisp_ident(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// A UI file's own top-level names, which get namespaced per file. `widgets`
/// is the `defwidget` subset: those are also named by string, as a box's
/// `:background "name"` or a tab's `:icon "name"`.
pub struct LocalHelpers {
    pub names: BTreeSet<String>,
    pub widgets: BTreeSet<String>,
}

pub fn local_helper_names(exprs: &[eseqlisp::parser::Expression]) -> LocalHelpers {
    use eseqlisp::parser::Expression;

    let mut helpers = LocalHelpers { names: BTreeSet::new(), widgets: BTreeSet::new() };
    for expr in exprs {
        let Expression::List(items) = expr else { continue };
        let (Some(Expression::Symbol(head)), Some(Expression::Symbol(name))) =
            (items.first(), items.get(1))
        else {
            continue;
        };
        match head.as_str() {
            "def" | "defmacro" => {}
            "defwidget" => {
                helpers.widgets.insert(name.clone());
            }
            _ => continue,
        }
        helpers.names.insert(name.clone());
    }
    helpers
}

pub fn namespace_local_helpers(
    expr: &eseqlisp::parser::Expression,
    helpers: &LocalHelpers,
    prefix: &str,
) -> eseqlisp::parser::Expression {
    use eseqlisp::parser::Expression;

    let rename_symbol = |name: &str| {
        if helpers.names.contains(name) {
            format!("{prefix}{}", safe_lisp_ident(name))
        } else {
            name.to_string()
        }
    };

    match expr {
        Expression::Symbol(name) => Expression::Symbol(rename_symbol(name)),
        Expression::Keyword(value) => Expression::Keyword(value.clone()),
        Expression::String(value) => Expression::String(value.clone()),
        Expression::QuoteSymbol(value) => Expression::QuoteSymbol(value.clone()),
        Expression::QuoteList(items) => Expression::QuoteList(items.clone()),
        Expression::Number(value) => Expression::Number(*value),
        // Macro templates emit executable forms. Their local helper references
        // need the same namespace as calls authored outside the template.
        // Plain quoted data above deliberately remains untouched.
        Expression::Quasiquote(inner) => Expression::Quasiquote(Box::new(
            namespace_local_helpers(inner, helpers, prefix),
        )),
        Expression::Unquote(inner) => {
            Expression::Unquote(Box::new(namespace_local_helpers(inner, helpers, prefix)))
        }
        Expression::UnquoteSplicing(inner) => Expression::UnquoteSplicing(Box::new(
            namespace_local_helpers(inner, helpers, prefix),
        )),
        Expression::List(items) => Expression::List(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| match (index.checked_sub(1).map(|i| &items[i]), item) {
                    // A local widget named by string follows its renamed definition.
                    (Some(Expression::Keyword(key)), Expression::String(name))
                        if matches!(key.as_str(), "background" | "icon")
                            && helpers.widgets.contains(name) =>
                    {
                        Expression::String(rename_symbol(name))
                    }
                    _ => namespace_local_helpers(item, helpers, prefix),
                })
                .collect(),
        ),
    }
}

/// One instrument's generated panel code: its namespaced helper definitions
/// and its `custom-synth-ui-<id>` function (named `fn_name`).
pub struct InstrumentUiDefinitions {
    pub helpers: Vec<String>,
    pub function: String,
    pub fn_name: String,
}

/// Transform one instrument's parsed `ui.lisp`; `None` without `defsynth-ui`.
pub fn instrument_ui_definitions(
    instrument_name: &str,
    exprs: &[eseqlisp::parser::Expression],
) -> Option<InstrumentUiDefinitions> {
    use eseqlisp::parser::Expression;

    let helper_prefix = format!("custom_ui_{}__", safe_lisp_ident(instrument_name));
    let helper_names = local_helper_names(exprs);
    let mut body = None;
    let mut helpers = Vec::new();
    for expr in exprs {
        let expr = namespace_local_helpers(expr, &helper_names, &helper_prefix);
        if let Expression::List(items) = &expr {
            if matches!(items.first(), Some(Expression::Symbol(head)) if head == "defsynth-ui") {
                body = items.get(1).map(transform_synth_ui_expr);
                continue;
            }
        }
        helpers.push(transform_synth_ui_expr(&expr));
    }
    let body = body?;
    let fn_name = format!("custom-synth-ui-{}", safe_lisp_ident(instrument_name));
    let function = format!(
        "(def {fn_name} (inst) (do (set! synth-ui-current-inst inst) (set! synth-ui-current-name {}) (set! custom-ui-current-kind \"instrument\") (set! custom-ui-selected-section (eseq.effects.custom-ui-sections/custom-ui-selected-section-for-current-scope)) {body}))",
        lisp_string_literal(instrument_name.trim_end_matches('/'))
    );
    Some(InstrumentUiDefinitions { helpers, function, fn_name })
}

/// The custom-UI source for one instrument (a plugin hosts exactly one): its
/// definitions and a `custom-instrument-synth-ui` that always answers with it.
pub fn single_instrument_ui_source(instrument_name: &str, ui_path: &str, src: &str) -> Result<String, String> {
    use eseqlisp::parser::{ASTParser, Parser};

    let tokens = Parser::new(src.to_string())
        .parse()
        .map_err(|err| format!("{ui_path}: parse error {err:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|err| format!("{ui_path}: {err:?}"))?;
    let removed = eseqlisp::compiler::removed_form_uses(&exprs);
    if !removed.is_empty() {
        return Err(format!("{ui_path} uses removed forms: {}", removed.join("; ")));
    }
    let definitions =
        instrument_ui_definitions(instrument_name, &exprs).ok_or_else(|| format!("{ui_path} has no defsynth-ui"))?;
    let mut out = String::new();
    for helper in &definitions.helpers {
        out.push_str(&format!("\n{helper}\n"));
    }
    out.push_str(&format!("\n{}\n", definitions.function));
    out.push_str(&format!("\n(def custom-instrument-synth-ui (inst) ({} inst))\n", definitions.fn_name));
    Ok(out)
}
