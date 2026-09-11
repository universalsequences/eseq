//! Static HTML export of the same manual AST used by the in-app reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use eseqlisp::manual::{self, Block, Inline, Page};

const STYLE: &str = include_str!("manual_export/style.css");
const MANIFEST: &str = "export-manifest.json";

struct Chapter {
    page: Page,
    images: BTreeMap<String, (u32, u32)>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&#39;")
}

fn node_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn url_path(path: &str) -> String {
    let mut url = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            url.push(byte as char);
        } else {
            write!(url, "%{byte:02X}").unwrap();
        }
    }
    url
}

fn href(target: &str, pages: &BTreeMap<String, Chapter>) -> Result<String, String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        Ok(escape(target))
    } else if pages.contains_key(target) {
        Ok(format!("{target}.html"))
    } else {
        Err(format!("unknown manual link target {target:?}"))
    }
}

fn render_inline(items: &[Inline], pages: &BTreeMap<String, Chapter>) -> Result<String, String> {
    let mut html = String::new();
    for item in items {
        let text = escape(item.plain_text());
        match item {
            Inline::Text(_) => html.push_str(&text),
            Inline::Bold(_) => write!(html, "<strong>{text}</strong>").unwrap(),
            Inline::Em(_) => write!(html, "<em>{text}</em>").unwrap(),
            Inline::Code(_) => write!(html, "<code>{text}</code>").unwrap(),
            Inline::Action { .. } => write!(html, "<span class=\"app-action\">{text}</span>").unwrap(),
            Inline::Link { target, .. } => {
                write!(html, "<a href=\"{}\">{text}</a>", href(target, pages)?).unwrap();
            }
        }
    }
    Ok(html)
}

fn menu_targets(page: &Page) -> Vec<&str> {
    page.blocks.iter().filter_map(|block| match block {
        Block::Menu(entries) => Some(entries.iter().map(|entry| entry.target.as_str())),
        _ => None,
    }).flatten().collect()
}

fn visit_chapters(name: &str, pages: &BTreeMap<String, Chapter>, order: &mut Vec<String>) {
    if order.iter().any(|visited| visited == name) { return; }
    order.push(name.to_string());
    for target in menu_targets(&pages[name].page) {
        visit_chapters(target, pages, order);
    }
}

fn chapter_links(current: &str, order: &[String], pages: &BTreeMap<String, Chapter>) -> String {
    let mut html = String::from("<ol class=\"chapter-list\">");
    for name in order {
        let title = if name == "index" { "Overview" } else { pages[name].page.title().unwrap() };
        let active = if name == current { " aria-current=\"page\"" } else { "" };
        write!(html, "<li><a href=\"{name}.html\"{active}>{}</a></li>", escape(title)).unwrap();
    }
    html.push_str("</ol>");
    html
}

fn heading_ids(page: &Page) -> Vec<String> {
    let mut used = BTreeSet::from(["content".to_string()]);
    page.blocks.iter().filter_map(|block| match block {
        Block::Heading { text, .. } => {
            let base = text.to_lowercase().split(|c: char| !c.is_alphanumeric())
                .filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
            let base = if base.is_empty() { "section".to_string() } else { base };
            let mut id = base.clone();
            let mut suffix = 2;
            while !used.insert(id.clone()) {
                id = format!("{base}-{suffix}");
                suffix += 1;
            }
            Some(id)
        }
        _ => None,
    }).collect()
}

fn render_page(name: &str, pages: &BTreeMap<String, Chapter>, order: &[String]) -> Result<String, String> {
    let chapter = &pages[name];
    let title = escape(chapter.page.title().unwrap());
    let links = chapter_links(name, order, pages);
    let mut html = format!(r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} · eseq manual</title>
  <link rel="stylesheet" href="manual.css">
</head>
<body>
  <a class="skip-link" href="#content">Skip to content</a>
  <header class="site-header"><a class="brand" href="../index.html">eseq<span> / manual</span></a><a href="../index.html">Back to eseq <span aria-hidden="true">↗</span></a></header>
  <div class="manual-shell">
    <aside class="sidebar"><nav aria-label="Chapters"><p class="nav-label">The manual</p>{links}</nav></aside>
    <div class="reading-pane">
      <details class="mobile-chapters"><summary>Browse chapters</summary><nav aria-label="Chapters">{links}</nav></details>
      <main id="content">
        <p class="eyebrow">eseq user manual</p>
"##);
    let ids = heading_ids(&chapter.page);
    let mut headings = ids.iter();
    for block in &chapter.page.blocks {
        match block {
            Block::Heading { level, text } => {
                let id = headings.next().unwrap();
                writeln!(html, "<h{level} id=\"{}\">{}</h{level}>", escape(id), escape(text)).unwrap();
            }
            Block::Paragraph(items) => writeln!(html, "<p>{}</p>", render_inline(items, pages)?).unwrap(),
            Block::Image { alt, src } => {
                let (width, height) = chapter.images[src];
                let url = escape(&url_path(src));
                writeln!(html, "<figure><a class=\"figure-link\" href=\"{url}\" aria-label=\"Open image at full size: {}\"><img src=\"{url}\" alt=\"{}\" width=\"{width}\" height=\"{height}\" loading=\"lazy\" decoding=\"async\"></a>", escape(alt), escape(alt)).unwrap();
                if !alt.is_empty() { writeln!(html, "<figcaption>{}</figcaption>", escape(alt)).unwrap(); }
                html.push_str("</figure>\n");
            }
            Block::CodeBlock { code, .. } => writeln!(html, "<pre><code>{}</code></pre>", escape(code)).unwrap(),
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                writeln!(html, "<{tag}>").unwrap();
                for item in items { writeln!(html, "<li>{}</li>", render_inline(item, pages)?).unwrap(); }
                writeln!(html, "</{tag}>").unwrap();
            }
            Block::Menu(entries) => {
                html.push_str("<ul class=\"chapter-menu\">\n");
                for entry in entries {
                    writeln!(html, "<li><a href=\"{}\">{}</a><p>{}</p></li>",
                        href(&entry.target, pages)?, escape(&entry.label), escape(&entry.description)).unwrap();
                }
                html.push_str("</ul>\n");
            }
        }
    }
    html.push_str("</main>\n<nav class=\"page-turn\" aria-label=\"Page navigation\">");
    // Prev/next follow the first parent menu's sibling order, as in the app.
    let siblings = order.iter().find_map(|parent| {
        let siblings = menu_targets(&pages[parent].page);
        siblings.iter().position(|target| *target == name).map(|index| (parent, siblings, index))
    });
    if let Some((parent, siblings, index)) = siblings {
        let previous = index.checked_sub(1).map(|index| siblings[index]).unwrap_or(parent);
        write!(html, "<a rel=\"prev\" href=\"{previous}.html\"><span>← Previous</span>{}</a>", escape(pages[previous].page.title().unwrap())).unwrap();
        if let Some(next) = siblings.get(index + 1) {
            write!(html, "<a rel=\"next\" href=\"{next}.html\"><span>Next →</span>{}</a>", escape(pages[*next].page.title().unwrap())).unwrap();
        }
    } else if name == "index" {
        if let Some(next) = menu_targets(&chapter.page).first() {
            write!(html, "<a rel=\"next\" href=\"{next}.html\"><span>Start reading →</span>{}</a>", escape(pages[*next].page.title().unwrap())).unwrap();
        }
    }
    html.push_str("</nav><footer>eseq manual <span aria-hidden=\"true\">·</span> <a href=\"index.html\">All chapters</a></footer></div></div>\n</body>\n</html>\n");
    Ok(html)
}

fn load_pages(source: &Path) -> Result<BTreeMap<String, Page>, String> {
    let mut pages = BTreeMap::new();
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") { continue; }
        let name = path.file_stem().and_then(|stem| stem.to_str()).ok_or("invalid page filename")?;
        if !node_name(name) { return Err(format!("invalid manual node name {name:?}")); }
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let page = manual::parse_manual_source(&text);
        if !matches!(page.blocks.first(), Some(Block::Heading { level: 1, .. }))
            || page.title().is_none_or(|title| title.trim().is_empty())
            || page.blocks.iter().filter(|block| matches!(block, Block::Heading { level: 1, .. })).count() != 1 {
            return Err(format!("{name}: a manual page must start with exactly one H1"));
        }
        pages.insert(name.to_string(), page);
    }
    if !pages.contains_key("index") { return Err("manual requires index.md".to_string()); }
    Ok(pages)
}

/// Discover figures using the reader's parser, without requiring existing
/// image files. A newly authored figure can therefore be generated first.
fn referenced_images(source: &Path) -> Result<BTreeSet<String>, String> {
    Ok(load_pages(source)?.into_values().flat_map(|page|
        page.blocks.into_iter().filter_map(|block| match block {
            Block::Image { src, .. } => Some(src),
            _ => None,
        })
    ).collect())
}

fn export(source: &Path, output: &Path) -> Result<(usize, usize), String> {
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    let mut pages = BTreeMap::new();
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    for (name, page) in load_pages(&source)? {
        let path = source.join(format!("{name}.md"));
        let mut images = BTreeMap::new();
        for block in &page.blocks {
            if let Block::Image { src, .. } = block {
                if !src.starts_with("images/") { return Err(format!("{name}: image must live under images/: {src}")); }
                let (file, width, height) = manual::image_info(&path, src)?;
                if !files.contains_key(src) {
                    files.insert(src.clone(), fs::read(file).map_err(|error| error.to_string())?);
                }
                images.insert(src.clone(), (width, height));
            }
        }
        pages.insert(name, Chapter { page, images });
    }
    for (name, chapter) in &pages {
        for target in menu_targets(&chapter.page) {
            if !pages.contains_key(target) { return Err(format!("{name}: missing menu page {target:?}")); }
        }
    }
    let mut order = Vec::new();
    visit_chapters("index", &pages, &mut order);
    for name in pages.keys() {
        if !order.contains(name) {
            eprintln!("warning: {name} has no menu path from index; appended to chapter navigation");
            order.push(name.clone());
        }
    }
    let image_count = files.len();
    for name in pages.keys() {
        let html = render_page(name, &pages, &order).map_err(|error| format!("{name}: {error}"))?;
        files.insert(format!("{name}.html"), html.into_bytes());
    }
    files.insert("manual.css".to_string(), STYLE.as_bytes().to_vec());

    // Validate every page and image before touching a previous export. Only
    // files recorded by this exporter can be replaced or removed on reruns.
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let output = output.canonicalize().map_err(|error| error.to_string())?;
    if output.starts_with(&source) { return Err("output must be outside the manual source directory".to_string()); }
    let manifest = output.join(MANIFEST);
    let previous: BTreeSet<String> = if manifest.exists() {
        serde_json::from_slice(&fs::read(&manifest).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid export manifest: {error}"))?
    } else { BTreeSet::new() };
    for path in &previous {
        if Path::new(path).components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
            return Err("export manifest contains an invalid relative path".to_string());
        }
    }
    for name in files.keys() {
        if output.join(name).exists() && !previous.contains(name) {
            return Err(format!("refusing to overwrite a file outside the export manifest: {name}"));
        }
    }
    let next: BTreeSet<_> = files.keys().cloned().collect();
    for (name, bytes) in files {
        let path = output.join(name);
        fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
        fs::write(path, bytes).map_err(|error| error.to_string())?;
    }
    for stale in previous.difference(&next) {
        let path = output.join(stale);
        if path.is_file() { fs::remove_file(path).map_err(|error| error.to_string())?; }
    }
    fs::write(manifest, serde_json::to_vec_pretty(&next).unwrap()).map_err(|error| error.to_string())?;
    Ok((pages.len(), image_count))
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut source = PathBuf::from("docs/manual");
    let mut output = None;
    let mut list_images = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => source = PathBuf::from(args.next().ok_or("--source requires a directory")?),
            "--out" => output = Some(PathBuf::from(args.next().ok_or("--out requires a directory")?)),
            "--list-images" => list_images = true,
            "-h" | "--help" => {
                println!("usage: eseqlisp_manual_export [--source docs/manual] (--out WEBSITE/manual | --list-images)");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    if list_images {
        if output.is_some() { return Err("--list-images cannot be combined with --out".to_string()); }
        println!("{}", serde_json::to_string_pretty(&referenced_images(&source)?)
            .map_err(|error| error.to_string())?);
        return Ok(());
    }
    let output = output.ok_or("--out is required (or use --list-images)")?;
    let (pages, images) = export(&source, &output)?;
    println!("Exported {pages} pages and {images} images to {}", output.display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("manual export: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "manual_export/tests.rs"]
mod tests;
