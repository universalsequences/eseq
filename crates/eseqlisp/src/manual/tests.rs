use super::inline::parse_inline;
use super::*;
use crate::runtime::Runtime;

fn text(s: &str) -> Inline {
    Inline::Text(s.to_string())
}

/// Debug-print a value; the Lisp printer quotes symbols as `'sym`, which
/// the spec's sketch omits, so strip the quote for readable assertions.
fn printed(value: &Value) -> String {
    format!("{value:?}").replace('\'', "")
}

fn sexpr(source: &str) -> String {
    printed(&page_to_value(&parse_manual_source(source)))
}

#[test]
fn headings_paragraphs_and_soft_wrap() {
    let page = parse_manual_source("# Title\n\nline one\nline two\n\n## Sub\n\n### Deep\n");
    assert_eq!(
        page.blocks,
        vec![
            Block::Heading {
                level: 1,
                text: "Title".into()
            },
            Block::Paragraph(vec![text("line one line two")]),
            Block::Heading {
                level: 2,
                text: "Sub".into()
            },
            Block::Heading {
                level: 3,
                text: "Deep".into()
            },
        ]
    );
    assert_eq!(page.title(), Some("Title"));
}

#[test]
fn h4_and_missing_space_are_paragraph_text() {
    let page = parse_manual_source("#### four\n\n#nospace\n");
    assert_eq!(
        page.blocks,
        vec![
            Block::Paragraph(vec![text("#### four")]),
            Block::Paragraph(vec![text("#nospace")]),
        ]
    );
    assert_eq!(page.title(), None);
}

#[test]
fn inline_styles() {
    assert_eq!(
        parse_inline("a **bold** and *em* and `C-x b` end"),
        vec![
            text("a "),
            Inline::Bold("bold".into()),
            text(" and "),
            Inline::Em("em".into()),
            text(" and "),
            Inline::Code("C-x b".into()),
            text(" end"),
        ]
    );
}

#[test]
fn unclosed_openers_are_literal() {
    assert_eq!(parse_inline("a **b c"), vec![text("a **b c")]);
    assert_eq!(parse_inline("a *b c"), vec![text("a *b c")]);
    assert_eq!(parse_inline("a `b c"), vec![text("a `b c")]);
    assert_eq!(parse_inline("a [b c"), vec![text("a [b c")]);
    assert_eq!(parse_inline("a [b](c"), vec![text("a [b](c")]);
    assert_eq!(parse_inline("a [b] c"), vec![text("a [b] c")]);
    assert_eq!(parse_inline("** ****"), vec![text("** ****")]);
    assert_eq!(parse_inline("[](x) [x]()"), vec![text("[](x) [x]()")]);
}

#[test]
fn inline_code_is_literal_inside() {
    assert_eq!(
        parse_inline("`**not bold** [x](y) \\*`"),
        vec![Inline::Code("**not bold** [x](y) \\*".into())]
    );
}

#[test]
fn no_nested_styles() {
    // The inner markers stay literal inside the bold run.
    assert_eq!(
        parse_inline("**a *b* c**"),
        vec![Inline::Bold("a *b* c".into())]
    );
    assert_eq!(
        parse_inline("[**x**](node)"),
        vec![Inline::Link {
            label: "**x**".into(),
            target: "node".into()
        }]
    );
}

#[test]
fn escapes() {
    assert_eq!(
        parse_inline("\\*not em\\* \\`tick \\[brk \\\\ \\n"),
        vec![text("*not em* `tick [brk \\ \\n")]
    );
}

#[test]
fn links_by_target_kind() {
    assert_eq!(
        parse_inline("[a](node-name) [b](https://x.y/z?q=(1)) [c](action:m-x choose-model)"),
        vec![
            Inline::Link {
                label: "a".into(),
                target: "node-name".into()
            },
            text(" "),
            Inline::Link {
                label: "b".into(),
                target: "https://x.y/z?q=(1)".into()
            },
            text(" "),
            Inline::Action {
                label: "c".into(),
                form: "m-x choose-model".into()
            },
        ]
    );
}

#[test]
fn action_link_keeps_nested_parens_verbatim() {
    assert_eq!(
        parse_inline("[go](action:switch-to-buffer (buffer-name (current)))!"),
        vec![
            Inline::Action {
                label: "go".into(),
                form: "switch-to-buffer (buffer-name (current))".into()
            },
            text("!"),
        ]
    );
}

#[test]
fn code_blocks_are_literal_and_unclosed_fence_swallows_rest() {
    let page = parse_manual_source("```lisp\n(seq-roll 1)\n# not heading\n```\nafter\n```\nopen\n");
    assert_eq!(
        page.blocks,
        vec![
            Block::CodeBlock {
                info: "lisp".into(),
                code: "(seq-roll 1)\n# not heading".into()
            },
            Block::Paragraph(vec![text("after")]),
            Block::CodeBlock {
                info: String::new(),
                code: "open".into()
            },
        ]
    );
}

#[test]
fn lists_with_continuations_and_renumbering() {
    let page = parse_manual_source("- one\n  more\n- two\n\n7. a\n9. b\ntrailing\n");
    assert_eq!(
        page.blocks,
        vec![
            Block::List {
                ordered: false,
                items: vec![vec![text("one more")], vec![text("two")]],
            },
            Block::List {
                ordered: true,
                items: vec![vec![text("a")], vec![text("b")]]
            },
            Block::Paragraph(vec![text("trailing")]),
        ]
    );
}

#[test]
fn switching_marker_kind_starts_a_new_list() {
    let page = parse_manual_source("- a\n1. b\n");
    assert_eq!(
        page.blocks,
        vec![
            Block::List {
                ordered: false,
                items: vec![vec![text("a")]]
            },
            Block::List {
                ordered: true,
                items: vec![vec![text("b")]]
            },
        ]
    );
}

#[test]
fn menu_classification() {
    let page = parse_manual_source(
        "- [Sequencer Tour](sequencer-tour) — step editing, `p-locks`\n- [Mixer](mixer)\n- [Keys](ref-keys): the key index\n",
    );
    assert_eq!(
        page.blocks,
        vec![Block::Menu(vec![
            MenuEntry {
                label: "Sequencer Tour".into(),
                target: "sequencer-tour".into(),
                description: "step editing, p-locks".into(),
            },
            MenuEntry {
                label: "Mixer".into(),
                target: "mixer".into(),
                description: String::new()
            },
            MenuEntry {
                label: "Keys".into(),
                target: "ref-keys".into(),
                description: "the key index".into()
            },
        ])]
    );
}

#[test]
fn menu_demotes_to_ul_when_any_item_is_not_a_cross_reference() {
    let mixed = parse_manual_source("- [a](a)\n- plain\n");
    assert!(matches!(
        mixed.blocks[0],
        Block::List { ordered: false, .. }
    ));
    let external = parse_manual_source("- [a](a)\n- [b](https://b)\n");
    assert!(matches!(
        external.blocks[0],
        Block::List { ordered: false, .. }
    ));
    let action = parse_manual_source("- [a](action:m-x foo)\n");
    assert!(matches!(
        action.blocks[0],
        Block::List { ordered: false, .. }
    ));
    let ordered = parse_manual_source("1. [a](a)\n");
    assert!(matches!(
        ordered.blocks[0],
        Block::List { ordered: true, .. }
    ));
}

#[test]
fn empty_and_whitespace_sources() {
    assert_eq!(parse_manual_source("").blocks, vec![]);
    assert_eq!(parse_manual_source("\n\n  \n").blocks, vec![]);
    assert_eq!(sexpr(""), "(page)");
}

#[test]
fn sexpr_shape_matches_spec() {
    let out = sexpr(
        "# Sequencer Tour\n\nSteps live in the [step buffer](step-buffer) with `p-locks`.\n\n## Rolls\n\n```lisp\n(seq-roll 1)\n```\n\n- **x** y\n\n1. z\n\n- [Mixer](mixer) — levels\n\n[open](action:switch-to-buffer *mixer*)\n",
    );
    assert_eq!(
        out,
        "(page (h1 \"Sequencer Tour\") (p (span \"Steps live in the \") (link \"step buffer\" \"step-buffer\") (span \" with \") (code \"p-locks\") (span \".\")) (h2 \"Rolls\") (code-block \"lisp\" \"(seq-roll 1)\") (ul (li (b \"x\") (span \" y\"))) (ol (li (span \"z\"))) (menu (entry \"Mixer\" \"mixer\" \"levels\")) (p (action-link \"open\" \"switch-to-buffer *mixer*\")))"
    );
}

#[test]
fn natives_parse_source_and_file() {
    let mut runtime = Runtime::new();
    crate::manual::register_manual_natives(&mut runtime);
    let value = runtime
        .eval_str("(parse-manual-source \"# T\n\nhi\")")
        .unwrap()
        .unwrap();
    assert_eq!(printed(&value), "(page (h1 \"T\") (p (span \"hi\")))");

    let dir = std::env::temp_dir().join(format!("eseq-manual-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("node.md");
    std::fs::write(&path, "# From Disk\n").unwrap();
    let form = format!("(parse-manual-page \"{}\")", path.display());
    let value = runtime.eval_str(&form).unwrap().unwrap();
    assert_eq!(printed(&value), "(page (h1 \"From Disk\"))");
    std::fs::remove_dir_all(&dir).unwrap();

    let missing = runtime.eval_str("(parse-manual-page \"/nonexistent/manual/x.md\")");
    // A native Err surfaces as `false` from eval_str in this runtime.
    assert_eq!(
        missing.unwrap(),
        Some(Value::Bool(false)),
        "unreadable file must error"
    );
}

#[test]
fn wrap_runs_glue_words_without_whitespace_between_fragments() {
    let inlines = parse_inline("Steps in the [step buffer](step-buffer), with `p locks`.**A**b");
    let groups = wrap_runs(&inlines);
    let flat: Vec<Vec<(&str, &str, Option<&str>)>> = groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|f| (f.kind, f.text.as_str(), f.target.as_deref()))
                .collect()
        })
        .collect();
    assert_eq!(
        flat,
        vec![
            vec![("span", "Steps", None)],
            vec![("span", "in", None)],
            vec![("span", "the", None)],
            vec![("link", "step", Some("step-buffer"))],
            vec![("link", "buffer", Some("step-buffer")), ("span", ",", None)],
            vec![("span", "with", None)],
            vec![
                ("code", "p locks", None),
                ("span", ".", None),
                ("b", "A", None),
                ("span", "b", None),
            ],
        ]
    );
}

#[test]
fn wrap_runs_native_round_trips_the_ast() {
    let mut runtime = Runtime::new();
    crate::manual::register_manual_natives(&mut runtime);
    let value = runtime
        .eval_str("(manual-wrap-runs (rest (nth (parse-manual-source \"a [b c](d) e\") 1)))")
        .unwrap()
        .unwrap();
    assert_eq!(
        printed(&value),
        "(((span \"a\")) ((link \"b\" \"d\")) ((link \"c\" \"d\")) ((span \"e\")))"
    );
}
