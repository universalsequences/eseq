# Manual HTML export

The app and website share the parser in `eseqlisp::manual`. To refresh the
manual in the sibling website repository, run from the `eseq` root:

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

The opening workspace map is an authored diagram in
`docs/manual/diagrams/workspace.svg`. Its panel relationships follow the session
layout in `content/ui/seq-layout.lisp`; it is intentionally simplified rather
than drawn to scale. Both readers use its PNG rendition. After editing the SVG,
regenerate the PNG with librsvg (`brew install librsvg` on macOS), then export:

```sh
rsvg-convert --width 2880 --height 1840 \
  --output docs/manual/images/workspace.png docs/manual/diagrams/workspace.svg
```

The other illustrations are real app captures; regenerate those with
`scripts/capture_manual_images.py` as described in `docs/metal-seq-ui-capture.md`.

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
```
