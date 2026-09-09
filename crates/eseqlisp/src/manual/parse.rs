//! Block-level parsing (spec §2.1, §2.2, §2.4, §2.5, §2.7).

use super::inline::{is_cross_reference, parse_inline, Inline};
use super::{Block, MenuEntry, Page};

enum Marker {
    Unordered,
    Ordered,
}

/// `- text` / `1. text` → (marker, text after the marker).
fn list_item(line: &str) -> Option<(Marker, &str)> {
    if let Some(rest) = line.strip_prefix("- ") {
        return Some((Marker::Unordered, rest));
    }
    if line == "-" {
        return Some((Marker::Unordered, ""));
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            return Some((Marker::Ordered, rest));
        }
    }
    None
}

/// `# text` … `### text` → (level, text). `####` and `#text` are not headings.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    let text = rest.strip_prefix(' ')?.trim();
    Some((hashes as u8, text))
}

fn fence(line: &str) -> Option<&str> {
    line.strip_prefix("```").map(str::trim)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn join_lines(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Trim the conventional ` — ` / ` - ` / `: ` separator off a menu description.
fn trim_description(text: &str) -> String {
    let text = text.trim();
    let text = ["—", "–", "-", ":"]
        .iter()
        .find_map(|sep| text.strip_prefix(sep))
        .unwrap_or(text);
    text.trim().to_string()
}

fn menu_entries(items: &[Vec<Inline>]) -> Option<Vec<MenuEntry>> {
    items
        .iter()
        .map(|item| match item.first() {
            Some(link @ Inline::Link { label, target }) if is_cross_reference(link) => {
                let description = item[1..].iter().map(Inline::plain_text).collect::<String>();
                Some(MenuEntry {
                    label: label.clone(),
                    target: target.clone(),
                    description: trim_description(&description),
                })
            }
            _ => None,
        })
        .collect()
}

fn finish_list(ordered: bool, items: Vec<Vec<Inline>>) -> Block {
    if !ordered {
        if let Some(entries) = menu_entries(&items) {
            return Block::Menu(entries);
        }
    }
    Block::List { ordered, items }
}

/// Parse a page's markdown source. Total: never fails.
pub fn parse_manual_source(source: &str) -> Page {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if is_blank(line) {
            i += 1;
            continue;
        }

        if let Some(info) = fence(line) {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && fence(lines[i]).is_none() {
                code.push(lines[i]);
                i += 1;
            }
            // An unclosed fence swallows the rest of the page (total parse).
            i += 1;
            blocks.push(Block::CodeBlock {
                info: info.to_string(),
                code: code.join("\n"),
            });
            continue;
        }

        if let Some((level, text)) = heading(line) {
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            i += 1;
            continue;
        }

        if let Some((marker, first)) = list_item(line) {
            let ordered = matches!(marker, Marker::Ordered);
            let mut items: Vec<Vec<Inline>> = Vec::new();
            let mut current = vec![first];
            i += 1;
            while let Some(&next) = lines.get(i) {
                if is_blank(next) || fence(next).is_some() || heading(next).is_some() {
                    break;
                }
                match list_item(next) {
                    Some((next_marker, text))
                        if matches!(next_marker, Marker::Ordered) == ordered =>
                    {
                        items.push(parse_inline(&join_lines(&current)));
                        current = vec![text];
                    }
                    Some(_) => break,
                    None if is_indented(next) => current.push(next),
                    None => break,
                }
                i += 1;
            }
            items.push(parse_inline(&join_lines(&current)));
            blocks.push(finish_list(ordered, items));
            continue;
        }

        let mut para = vec![line];
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            if is_blank(next)
                || fence(next).is_some()
                || heading(next).is_some()
                || list_item(next).is_some()
            {
                break;
            }
            para.push(next);
            i += 1;
        }
        blocks.push(Block::Paragraph(parse_inline(&join_lines(&para))));
    }

    Page { blocks }
}
