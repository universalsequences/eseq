//! Word runs for the renderer (spec §4): split a paragraph's inline nodes
//! on whitespace into *groups* of fragments that must stay glued together
//! (no gap between them), so a `wrap` container can flow one widget per
//! group and word-wrap around styled fragments without ragged spacing.
//!
//! `"in the "` + `(link "step buffer" …)` + `"."` becomes the groups
//! `[in] [the] [step→] [buffer→ .]`: the trailing `.` glues onto the last
//! link word because no whitespace separates them.

use super::inline::Inline;

/// One fragment of a group: a style kind, its text, and a link/action
/// target when the kind carries one.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub kind: &'static str,
    pub text: String,
    pub target: Option<String>,
}

fn fragment(kind: &'static str, text: &str, target: Option<&str>) -> Fragment {
    Fragment {
        kind,
        text: text.to_string(),
        target: target.map(str::to_string),
    }
}

/// Split inline nodes into glued groups of word fragments.
pub fn wrap_runs(inlines: &[Inline]) -> Vec<Vec<Fragment>> {
    let mut groups: Vec<Vec<Fragment>> = Vec::new();
    // True while the next fragment attaches to the current group.
    let mut glue = false;

    let push = |groups: &mut Vec<Vec<Fragment>>, glue: &mut bool, frag: Fragment| {
        match groups.last_mut() {
            Some(last) if *glue => last.push(frag),
            _ => groups.push(vec![frag]),
        }
        *glue = true;
    };

    for inline in inlines {
        let (kind, text, target): (&'static str, &str, Option<&str>) = match inline {
            Inline::Text(s) => ("span", s, None),
            Inline::Bold(s) => ("b", s, None),
            Inline::Em(s) => ("em", s, None),
            Inline::Code(s) => {
                // Code stays one chip; its inner spaces are literal.
                push(&mut groups, &mut glue, fragment("code", s, None));
                continue;
            }
            Inline::Link { label, target } => ("link", label, Some(target)),
            Inline::Action { label, form } => ("action-link", label, Some(form)),
        };
        if text.is_empty() {
            continue;
        }
        if text.starts_with(char::is_whitespace) {
            glue = false;
        }
        for (index, word) in text.split_whitespace().enumerate() {
            if index > 0 {
                glue = false;
            }
            push(&mut groups, &mut glue, fragment(kind, word, target));
        }
        if text.ends_with(char::is_whitespace) {
            glue = false;
        }
    }
    groups
}
