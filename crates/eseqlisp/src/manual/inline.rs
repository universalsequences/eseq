//! Inline parsing (spec §2.3, §2.6, §3): one paragraph's text to a flat
//! list of styled fragments. No nesting; an unclosed opener is literal.

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(String),
    Em(String),
    Code(String),
    /// `[label](target)` where target is a node name or an http(s) URL.
    Link {
        label: String,
        target: String,
    },
    /// `[label](action:form)`; `form` is the raw text after `action:`,
    /// without outer parens, stored verbatim and never evaluated here.
    Action {
        label: String,
        form: String,
    },
}

impl Inline {
    /// The fragment's text with styling dropped (used for menu descriptions).
    pub fn plain_text(&self) -> &str {
        match self {
            Inline::Text(s) | Inline::Bold(s) | Inline::Em(s) | Inline::Code(s) => s,
            Inline::Link { label, .. } | Inline::Action { label, .. } => label,
        }
    }
}

const ESCAPABLE: [char; 4] = ['*', '`', '[', '\\'];

pub fn parse_inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut run = String::new();
    let mut i = 0;

    let flush = |run: &mut String, out: &mut Vec<Inline>| {
        if !run.is_empty() {
            out.push(Inline::Text(std::mem::take(run)));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && ESCAPABLE.contains(&chars[i + 1]) => {
                run.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                if let Some(end) = find(&chars, i + 1, |j| chars[j] == '`') {
                    flush(&mut run, &mut out);
                    out.push(Inline::Code(collect(&chars, i + 1, end)));
                    i = end + 1;
                } else {
                    run.push(c);
                    i += 1;
                }
            }
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                let start = i + 2;
                let close = find(&chars, start, |j| {
                    chars[j] == '*' && j + 1 < chars.len() && chars[j + 1] == '*'
                });
                match close {
                    Some(end) if has_content(&chars, start, end) => {
                        flush(&mut run, &mut out);
                        out.push(Inline::Bold(collect(&chars, start, end)));
                        i = end + 2;
                    }
                    _ => {
                        run.push_str("**");
                        i += 2;
                    }
                }
            }
            '*' => {
                let start = i + 1;
                let close = find(&chars, start, |j| chars[j] == '*');
                match close {
                    Some(end) if has_content(&chars, start, end) => {
                        flush(&mut run, &mut out);
                        out.push(Inline::Em(collect(&chars, start, end)));
                        i = end + 1;
                    }
                    _ => {
                        run.push(c);
                        i += 1;
                    }
                }
            }
            '[' => match parse_link(&chars, i) {
                Some((inline, next)) => {
                    flush(&mut run, &mut out);
                    out.push(inline);
                    i = next;
                }
                None => {
                    run.push(c);
                    i += 1;
                }
            },
            _ => {
                run.push(c);
                i += 1;
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `[label](target)` at `open`; returns the node and the index after `)`.
/// The label may not contain `[`/`]`; the target allows balanced inner
/// parens (nested Lisp forms in action links) but no newlines.
fn parse_link(chars: &[char], open: usize) -> Option<(Inline, usize)> {
    let label_end = find(chars, open + 1, |j| chars[j] == ']' || chars[j] == '[')?;
    if chars[label_end] != ']' || chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let target_start = label_end + 2;
    let mut depth = 0usize;
    let mut j = target_start;
    let target_end = loop {
        let c = *chars.get(j)?;
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => break j,
            ')' => depth -= 1,
            '\n' => return None,
            _ => {}
        }
        j += 1;
    };
    let label = collect(chars, open + 1, label_end);
    let target = collect(chars, target_start, target_end);
    if label.is_empty() || target.is_empty() {
        return None;
    }
    let inline = match target.strip_prefix("action:") {
        Some(form) => Inline::Action {
            label,
            form: form.trim().to_string(),
        },
        None => Inline::Link { label, target },
    };
    Some((inline, target_end + 1))
}

/// A cross-reference link: a bare node name, neither external nor an action.
pub fn is_cross_reference(inline: &Inline) -> bool {
    match inline {
        Inline::Link { target, .. } => {
            !(target.starts_with("http://") || target.starts_with("https://"))
        }
        _ => false,
    }
}

/// A style run must contain something other than whitespace.
fn has_content(chars: &[char], start: usize, end: usize) -> bool {
    end > start && chars[start..end].iter().any(|c| !c.is_whitespace())
}

fn find(chars: &[char], from: usize, pred: impl Fn(usize) -> bool) -> Option<usize> {
    (from..chars.len()).find(|&j| pred(j))
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect()
}
