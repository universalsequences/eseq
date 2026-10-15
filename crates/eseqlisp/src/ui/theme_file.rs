//! Reading and rewriting theme source files for the theme editor.
//!
//! A theme file is ordinary Lisp whose payload is one
//! `(apply-theme (dict :slot value ...))` form. Saving must not reformat the
//! file: comments, ordering and alignment are hand-tuned. So instead of
//! printing the dict, [`ThemeDict::scan`] finds the byte span of every
//! `:slot value` pair and edits replace only the value text; slots the file
//! does not mention yet are appended after the last entry, indented like it.

use crate::backend::Color;

/// One `:slot value` pair inside the theme dict, by byte span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeEntry {
    /// Keyword name without the colon (`status-ui-bg`).
    pub name: String,
    pub key_start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

/// The `(dict ...)` inside a theme file's `apply-theme` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDict {
    pub entries: Vec<ThemeEntry>,
    /// Byte offset of the dict's closing paren.
    pub close: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Quote,
    /// An atom, string or other self-delimiting token.
    Atom,
}

/// Byte-span tokens of `text`, skipping whitespace and `;` comments.
fn tokenize(text: &str) -> Vec<(Token, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b if b.is_ascii_whitespace() => i += 1,
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' | b'[' => {
                out.push((Token::Open, i, i + 1));
                i += 1;
            }
            b')' | b']' => {
                out.push((Token::Close, i, i + 1));
                i += 1;
            }
            b'\'' => {
                out.push((Token::Quote, i, i + 1));
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                out.push((Token::Atom, start, i));
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b')' | b'[' | b']' | b';' | b'"' | b'\'')
                {
                    i += 1;
                }
                out.push((Token::Atom, start, i));
            }
        }
    }
    out
}

/// The token index just past the form starting at `tokens[at]`.
fn form_end(tokens: &[(Token, usize, usize)], at: usize) -> Option<usize> {
    match tokens.get(at)?.0 {
        Token::Atom => Some(at + 1),
        Token::Quote => form_end(tokens, at + 1),
        Token::Close => None,
        Token::Open => {
            let mut depth = 0usize;
            for (offset, (token, _, _)) in tokens[at..].iter().enumerate() {
                match token {
                    Token::Open => depth += 1,
                    Token::Close => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + offset + 1);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
    }
}

impl ThemeDict {
    /// Locate the dict passed to the first `apply-theme` in `text`.
    pub fn scan(text: &str) -> Result<ThemeDict, String> {
        let tokens = tokenize(text);
        let atom = |index: usize| -> Option<&str> {
            match tokens.get(index) {
                Some((Token::Atom, start, end)) => Some(&text[*start..*end]),
                _ => None,
            }
        };
        let apply = (0..tokens.len())
            .find(|&i| tokens[i].0 == Token::Open && atom(i + 1) == Some("apply-theme"))
            .ok_or("no (apply-theme ...) form in the theme file")?;
        let dict_open = apply + 2;
        if tokens.get(dict_open).map(|t| t.0) != Some(Token::Open) || atom(dict_open + 1) != Some("dict") {
            return Err("apply-theme's argument is not a literal (dict ...)".to_string());
        }
        let dict_end = form_end(&tokens, dict_open).ok_or("unbalanced (dict ...) form")?;
        let close = tokens[dict_end - 1].1;
        let mut entries = Vec::new();
        let mut i = dict_open + 2;
        while i < dict_end - 1 {
            let key = atom(i).filter(|key| key.starts_with(':'));
            let value_at = i + 1;
            let value_end = form_end(&tokens, value_at).filter(|end| *end < dict_end);
            match (key, value_end) {
                (Some(key), Some(end)) => {
                    entries.push(ThemeEntry {
                        name: key[1..].to_string(),
                        key_start: tokens[i].1,
                        value_start: tokens[value_at].1,
                        value_end: tokens[end - 1].2,
                    });
                    i = end;
                }
                _ => {
                    return Err(format!(
                        "unexpected form in the theme dict near byte {}",
                        tokens[i].1
                    ));
                }
            }
        }
        Ok(ThemeDict { entries, close })
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.name.clone()).collect()
    }
}

/// A number as theme files write it: up to three decimals, at least one.
fn format_component(value: f32) -> String {
    let text = format!("{:.3}", value.clamp(0.0, 1.0));
    let trimmed = text.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_string()
    }
}

/// `'(r g b a)`, the quoted-list form theme files use.
pub fn format_color(color: Color) -> String {
    format!(
        "'({} {} {} {})",
        format_component(color.r),
        format_component(color.g),
        format_component(color.b),
        format_component(color.a)
    )
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |newline| newline + 1)
}

/// `text` with each `(slot, color)` written into its theme dict: existing
/// entries get their value replaced in place, the rest are appended after
/// the last entry (or at the dict's close when it is empty).
pub fn apply_color_edits(text: &str, edits: &[(String, Color)]) -> Result<String, String> {
    let dict = ThemeDict::scan(text)?;
    // (byte range to replace, replacement), applied back to front.
    let mut splices: Vec<(usize, usize, String)> = Vec::new();
    let mut appended = String::new();
    let last = dict.entries.last();
    let (indent, value_column) = match last {
        Some(entry) => {
            let start = line_start(text, entry.key_start);
            let indent: String = text[start..entry.key_start]
                .chars()
                .map(|c| if c == '\t' { '\t' } else { ' ' })
                .collect();
            (indent, entry.value_start - start)
        }
        None => ("      ".to_string(), 0),
    };
    for (name, color) in edits {
        let value = format_color(*color);
        if let Some(entry) = dict.entries.iter().find(|entry| entry.name == *name) {
            splices.push((entry.value_start, entry.value_end, value));
        } else {
            let key = format!("{indent}:{name}");
            let pad = value_column.saturating_sub(key.chars().count()).max(1);
            appended.push('\n');
            appended.push_str(&key);
            appended.push_str(&" ".repeat(pad));
            appended.push_str(&value);
        }
    }
    if !appended.is_empty() {
        let at = last.map_or(dict.close, |entry| {
            // After the last value and any comment trailing it on its line.
            let line_end = text[entry.value_end..]
                .find('\n')
                .map_or(text.len(), |offset| entry.value_end + offset);
            if text[entry.value_end..line_end].trim_start().starts_with(';') {
                line_end
            } else {
                entry.value_end
            }
        });
        let at = at.min(dict.close);
        splices.push((at, at, appended));
    }
    splices.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = text.to_string();
    for (start, end, replacement) in splices {
        out.replace_range(start..end, &replacement);
    }
    Ok(out)
}

/// Rename the symbol a theme file defines with its leading `(def NAME`, so a
/// copy saved under a new name does not redefine the original's function.
pub fn rename_theme_def(text: &str, new_name: &str) -> String {
    let tokens = tokenize(text);
    let def_name = tokens.windows(3).find_map(|window| {
        let [(Token::Open, _, _), (Token::Atom, ds, de), (Token::Atom, ns, ne)] = window else {
            return None;
        };
        (&text[*ds..*de] == "def").then(|| &text[*ns..*ne])
    });
    let Some(old) = def_name else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (token, start, end) in &tokens {
        if *token == Token::Atom && &text[*start..*end] == old {
            out.push_str(&text[cursor..*start]);
            out.push_str(new_name);
            cursor = *end;
        }
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = "\
;; demo theme
(def demo-theme ()
  (apply-theme (dict
      :bg                 '(0.1 0.2 0.3 1.0)  ; the background
      :text-buffer-bg :transparent
      :fg   '(0.9 0.9 0.9)
      )))

(demo-theme)
";

    #[test]
    fn scan_finds_every_entry_and_skips_comments() {
        let dict = ThemeDict::scan(THEME).unwrap();
        assert_eq!(dict.names(), vec!["bg", "text-buffer-bg", "fg"]);
        let bg = &dict.entries[0];
        assert_eq!(&THEME[bg.value_start..bg.value_end], "'(0.1 0.2 0.3 1.0)");
        assert_eq!(&THEME[dict.close..dict.close + 1], ")");
    }

    #[test]
    fn edits_replace_values_in_place_and_keep_comments() {
        let out = apply_color_edits(
            THEME,
            &[
                ("bg".to_string(), Color::rgba(0.5, 0.25, 0.125, 1.0)),
                ("text-buffer-bg".to_string(), Color::rgba(0.0, 0.0, 0.0, 0.5)),
            ],
        )
        .unwrap();
        assert!(out.contains(":bg                 '(0.5 0.25 0.125 1.0)  ; the background"));
        assert!(out.contains(":text-buffer-bg '(0.0 0.0 0.0 0.5)"));
        assert!(out.contains(":fg   '(0.9 0.9 0.9)"));
        assert_eq!(ThemeDict::scan(&out).unwrap().names().len(), 3);
    }

    #[test]
    fn missing_slots_are_appended_after_the_last_entry() {
        let out = apply_color_edits(
            THEME,
            &[("accent".to_string(), Color::rgba(1.0, 0.5, 0.0, 1.0))],
        )
        .unwrap();
        let dict = ThemeDict::scan(&out).unwrap();
        assert_eq!(dict.names(), vec!["bg", "text-buffer-bg", "fg", "accent"]);
        assert!(out.contains("      :fg   '(0.9 0.9 0.9)\n      :accent '(1.0 0.5 0.0 1.0)\n      )))"));
    }

    #[test]
    fn appending_after_a_commented_last_entry_keeps_the_comment_on_its_line() {
        let text = "(apply-theme (dict\n  :bg '(0 0 0)  ; note\n  ))";
        let out = apply_color_edits(text, &[("fg".to_string(), Color::rgba(1.0, 1.0, 1.0, 1.0))]).unwrap();
        assert_eq!(out, "(apply-theme (dict\n  :bg '(0 0 0)  ; note\n  :fg '(1.0 1.0 1.0 1.0)\n  ))");
    }

    /// The editor's Save must work on every shipped theme: each one scans,
    /// and rewriting all of its slots in place keeps the same slot list.
    #[test]
    fn every_factory_theme_scans_and_round_trips() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/ui/themes");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("themes dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("lisp") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let dict = ThemeDict::scan(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(dict.entries.len() > 100, "{}", path.display());
            let edits: Vec<(String, Color)> = dict
                .names()
                .into_iter()
                .map(|name| (name, Color::rgba(0.5, 0.5, 0.5, 1.0)))
                .collect();
            let out = apply_color_edits(&text, &edits).unwrap();
            assert_eq!(ThemeDict::scan(&out).unwrap().names(), dict.names(), "{}", path.display());
            checked += 1;
        }
        assert!(checked >= 10);
    }

    #[test]
    fn rename_replaces_the_def_and_its_call() {
        let out = rename_theme_def(THEME, "copy-theme");
        assert!(out.contains("(def copy-theme ()"));
        assert!(out.contains("\n(copy-theme)\n"));
        assert!(!out.contains("demo-theme ("));
    }
}
