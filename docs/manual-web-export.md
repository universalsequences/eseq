# Manual HTML export

## Refresh images and HTML together

After changing the interface or manual, run:

```sh
./scripts/refresh_manual.py
```

The command builds the current `metal_seq` and `eseqlisp_manual_export` in
release mode, discovers every image referenced by `docs/manual/*.md` using the
same parser as the app, and generates each unique image once. UI figures use
real headless Metal captures; authored SVG diagrams render to PNG at twice
their intrinsic size. The resulting PNGs replace their counterparts in
`docs/manual/images/`, and HTML plus identical image files go to
`../eseq-site/manual/`. Both readers keep the existing relative image paths.

It works from any working directory. The default website location is relative
to this repository. Override the HTML destination with `--out /path/to/manual`,
or use `--dev` to build both tools with Cargo's dev profile. Every run invokes
Cargo so capture never silently uses an outdated executable.

Prerequisites are macOS with Metal, this repository's normal Rust/build setup,
Python 3.9 or newer, and librsvg (`brew install librsvg`). Captures open no app
window or audio device. No Python packages are needed.

All figures are generated in a temporary staging directory before either
reader's assets are updated. Missing recipes, failed captures, invalid links,
and export ownership conflicts stop the run with a nonzero exit status,
leaving the previous source images and HTML files intact. Source PNGs are
replaced individually after successful export. Publishing across the two
directories is not one filesystem transaction: a disk/write failure during
publishing requires fixing the error and rerunning. Unreferenced source images
and unrelated website files are preserved.

## Add or change an illustration

Reference a PNG in the Markdown, for example `![Caption](images/my-panel.png)`.
Then provide exactly one source:

- For an app screenshot, add a named entry to
  `crates/sequencer/ui/capture-fixtures/manual-images.json`. Its `name` is
  `my-panel` (without `.png`), `script` names a project fixture beside the
  manifest, `buffer` chooses the panel, `key` chooses a subtree (or `null` for
  the whole buffer), and `width`/`height` set the surrounding layout viewport.
  See [the capture guide](metal-seq-ui-capture.md) for fixtures and cropping.
- For a diagram, author `docs/manual/diagrams/my-panel.svg`. The companion
  convention maps `images/my-panel.png` to `diagrams/my-panel.svg`; no extra
  manifest entry is needed. The workspace map uses this route. Its panel
  relationships follow `content/ui/seq-layout.lisp`.

The refresh command requires a generation source even if an old PNG exists,
and rejects an image with both sources. Only figures actually referenced by
the manual are generated. Inspect the resulting images after changing UI or
fixtures: successful capture validates geometry, not editorial usefulness.

For a quick subset of screenshots without an HTML export, keep using:

```sh
python3 scripts/capture_manual_images.py step-grid piano-roll
```

## Export existing images only

The app and website share the parser in `eseqlisp::manual`. To export HTML
without recapturing any images, run from the `eseq` root:

```sh
cargo run -p eseqlisp --bin eseqlisp_manual_export -- \
  --source docs/manual --out ../eseq-site/manual
```

The result is ordinary static HTML: one file per Markdown page, a shared
`manual.css`, and the referenced images with their original names and bytes.
Every URL is relative, so it works under a website subdirectory or directly
from disk. The header links back to the website's `../index.html`.

The sidebar follows the menu graph starting at `index`; Previous and Next
follow sibling menu order, as in the app. Pages without a path from the index
produce a warning and are appended to the sidebar. Small screens use a native
collapsible chapter menu. Clicking an illustration opens the original image.
App action labels render as inert text; Lisp forms never appear in the HTML
and are never evaluated. HTML metacharacters are escaped in all authored text.

The export validates page names, headings, link targets, and every image before
writing output. A missing page or asset fails the export, leaving the previous
HTML intact. `export-manifest.json` records generated files: subsequent exports
may replace those files and remove obsolete ones. Files outside that manifest
are preserved; collisions fail explicitly. Use a dedicated output directory.

The stylesheet lives in `crates/eseqlisp/src/bin/manual_export/style.css`.
Change that source rather than the generated copy. There are no web framework,
network, or JavaScript dependencies.

Serve the website locally:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory ../eseq-site
```

Then open <http://127.0.0.1:8765/manual/>.

The focused exporter tests cover shared AST rendering, image copying, relative
navigation, escaping and inert actions, validation before writes, and safe
regeneration:

```sh
cargo nextest run -p eseqlisp --bin eseqlisp_manual_export -E 'test(/tests::/)'
python3 scripts/test_refresh_manual.py
```
