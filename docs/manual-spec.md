# Manual Format — markdown subset, link conventions, `docs/manual/` layout

Status: rev 1, 2026-08-28. Spec bead: `eseq-ug3m.1`. Parent epic:
`eseq-ug3m` (in-app manual + web export, Info-style).

One markdown-subset source tree in `docs/manual/` is rendered three ways:

1. **In-app** — `parse-manual-page` (Rust native, `eseq-ug3m.2`) returns a
   Lisp AST; `content/ui/manual.lisp` renders it to widgets in the
   `*manual*` buffer (`eseq-ug3m.3`).
2. **Web** — a static-site exporter walks the same AST (`eseq-ug3m.6`).
3. **GitHub / plain markdown** — the files should remain readable as
   ordinary markdown with no custom tooling. Every extension below is
   chosen to degrade gracefully there.

The subset is deliberately tiny. Anything not listed in §2 is **not
supported**, and the whole point of writing this down is to keep the
in-app renderer and the web exporter honest: if a construct is tempting
but absent here, the fix is a spec revision, not a quiet parser feature.

## 1. Pages and nodes

- A **node** is one `.md` file in `docs/manual/`. Node name = filename
  without extension, kebab-case, `[a-z0-9-]+` (e.g. `sequencer-tour`).
- `index.md` is the root node ("Top" in Info terms). It is mostly a
  menu (§2.7) over the chapters.
- The directory is **flat** — no subdirectories. The manual is a graph
  of nodes linked by menus and cross-references, not a file hierarchy;
  a flat namespace keeps link targets unambiguous and renaming honest.
- A page **must** begin with exactly one `# H1` line; it is the node's
  display title (used by menus that omit a label, browser `<title>`,
  and the in-app header). No YAML front matter.
- Sibling order and next/prev navigation (`n`/`p` in the `*manual*`
  buffer) are **derived from the parent node's menu order**, not from
  filenames. A node reached by no menu is an orphan; the exporter warns.

## 2. The markdown subset

### 2.1 Headings

`#`, `##`, `###` (ATX only, no setext, no `####`+). One `#` per page
(§1). Headings are plain text — no emphasis, code, or links inside.

### 2.2 Paragraphs

Blank-line-separated runs of text. A single newline inside a paragraph
is a space (soft wrap). No hard-break syntax (trailing spaces mean
nothing).

### 2.3 Inline styles

Inside paragraphs and list items only:

- `**bold**`
- `*emphasis*`
- `` `inline code` `` — everything between the backticks is literal.
  Also the convention for key chords: `` `C-x b` ``, rendered
  monospace in-app.
- links (§3)

No nesting of inline styles (no bold-inside-link, no em-inside-bold).
A style opener with no closer on the same paragraph is literal text.

### 2.4 Code blocks

Fenced with triple backticks, optional info string (`lisp` is the
usual one; the reserved info string `menu` is **not** used — see §2.7).
Contents are fully literal. No indented code blocks.

### 2.5 Lists

`- ` unordered and `1. ` ordered (ordered markers are re-numbered on
render). One level only — **no nested lists**. An item is a single
paragraph; continuation lines indented to the item's text column belong
to the item.

### 2.6 Escaping

Backslash escapes exactly `` \* \` \[ \\ `` in inline text. Inside
inline code and code blocks nothing is special except the closing
delimiter.

### 2.7 Menu blocks (Info-style TOC)

There is no new syntax. **A bullet list in which every item begins with
a cross-reference link is a menu.** Item form:

```
- [Sequencer Tour](sequencer-tour) — step editing, p-locks, rolls
- [Mixer](mixer) — levels, sends, groups
```

Everything after the link (conventionally ` — description`, but any
inline text) is the entry's description. The parser classifies such a
list as `(menu …)` rather than `(ul …)`; the renderer gives it
Info-menu treatment (one entry per row, link + dimmed description) and
it defines child order for §1 next/prev navigation. On GitHub it is
just a list of links — which is exactly the intended degradation.

If even one item of a list does not begin with a link, the whole list
is an ordinary `ul`/`ol`. An ordinary link-only list being promoted to
a menu is harmless: entries stay clickable either way.

### 2.8 Explicitly excluded

Images, tables, blockquotes, HTML (inline or block), thematic breaks,
reference-style links, autolinks, footnotes, strikethrough, nested
lists, nested inline styles, setext headings, indented code blocks.
Wanting one of these = revising this spec first.

## 3. Links

All links use inline form `[label](target)`. The target string decides
the kind:

| target shape | kind | in-app | web export |
| --- | --- | --- | --- |
| `node-name` | cross-reference | navigate `*manual*` to that node | `<a href="node-name.html">` |
| `https://…` / `http://…` | external | open system browser | normal `<a>`, new tab |
| `action:…` (§3.1) | app action | execute the Lisp form | **plain styled text**, no link affordance |

Cross-reference targets are bare node names — no `.md` suffix, no
path, no `#fragment` (within-page anchors are out of scope for v0.1;
link to the node and let the reader scroll). The parser resolves
nothing; dangling cross-references are a lint the exporter and an
in-app check report, not a parse error.

### 3.1 Action links (app-only extension)

```markdown
[open the mixer](action:switch-to-buffer *mixer*)
[try it](action:m-x choose-model)
```

- Target = `action:` followed by a Lisp form **without its outer
  parens** (parens in a markdown link destination are hostile; the
  balanced inner parens of nested forms are allowed).
- At click time the in-app renderer wraps the text in one pair of
  parens and hands it to the reader/evaluator:
  `action:switch-to-buffer *mixer*` ⇒ `(switch-to-buffer *mixer*)`.
  Evaluation happens on click, never at parse or render time.
- The parser stores the raw text verbatim in a **distinct AST node**
  (§4); it does not read or validate the Lisp.
- On the web (and on GitHub) an action link degrades to plain styled
  text: the exporter emits the label as a styled `<span>` — same
  visual weight as body text, no underline/pointer — because a dead
  link is worse than no link.
- Authoring rule: an action link's surrounding sentence must still
  make sense as prose when the link is inert (e.g. "open the mixer
  (`C-x b *mixer*`)" style redundancy is encouraged).

## 4. AST seam

`parse-manual-page` (bead `eseq-ug3m.2`) returns an s-expr; this shape
is the shared seam for the in-app renderer, the web exporter, and any
later plain-text renderer. Normative sketch — node heads are fixed,
exact list shapes may be refined by `.2`/`.3` together:

```lisp
(page
  (h1 "Sequencer Tour")
  (p (span "Steps are edited in the ")
     (link "step buffer" "step-buffer")
     (span " with ")
     (code "p-locks")
     (span "."))
  (h2 "Rolls")
  (code-block "lisp" "(seq-roll …)")
  (ul (li (span "…")) …)
  (ol (li …) …)
  (menu
    (entry "Mixer" "mixer" "levels, sends, groups")
    …)
  (action-link "open the mixer" "switch-to-buffer *mixer*"))
```

- Inline content is a list of `span` / `b` / `em` / `code` / `link` /
  `action-link` nodes. `link` = `(link label target)`; `action-link`
  carries the raw form text from §3.1.
- Plain-text runs must be **splittable per word** (multiple `span`s or
  a renderer-side split — decided with `.3`) so word wrapping isn't
  ragged around styled fragments.
- **Malformed input never fails the parse.** Anything unrecognized
  falls back to literal paragraph text; the parser is total.

## 5. `docs/manual/` layout and generated reference nodes

```
docs/manual/
  index.md            # root: title + master menu
  concepts.md         # buffers, tiles, keys, M-x
  sequencer-tour.md
  arrangement.md
  mixer.md
  patcher.md
  sample-browser.md
  sound-design.md
  customization.md
```

(Chapter list is illustrative; the content bead `eseq-ug3m.5` owns the
real one. The layout rules are what's normative.)

**Generated reference nodes** (bead `eseq-ug3m.4` — key index, function
index, customize) live in the reserved node namespace **`ref-*`**:
`ref-keys`, `ref-functions`, `ref-customize`.

- Hand-written files must not use the `ref-` prefix, and generated
  nodes are **never checked into `docs/manual/`** — they can't rot in
  git if they don't live there.
- The generator emits them *in this same markdown subset*, so both
  consumers share one path: the web exporter materializes them into
  its build directory alongside the exported chapters; in-app they may
  be generated live from runtime metadata and fed straight to
  `parse-manual-page`.
- Hand-written chapters may cross-reference them like any node
  (`[key index](ref-keys)`); the dangling-link lint (§3) knows the
  reserved names exist even though the files don't.

`index.md`'s master menu should include the `ref-*` nodes so they are
reachable and ordered like everything else.

## 6. Lint (exporter + optional in-app check)

Not a parse concern (§4: the parser is total), but the exporter fails
and an in-app check can warn on:

- dangling cross-reference targets (unknown node, `ref-*` allowlisted)
- orphan nodes (reachable from no menu)
- missing or multiple `# H1`
- a `ref-*` filename appearing in `docs/manual/`
