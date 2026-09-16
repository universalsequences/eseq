# Factory instrument browser categories

Factory instrument directories may contain `.categories.json` to group their
immediate instrument children in the browser. Categories are display folders;
instrument IDs, DSP/UI paths, preset banks and user preset overlays keep their
existing locations. Synths has no category file and stays one level deep.

```json
{
  "version": 1,
  "groups": [
    {"label": "Kicks", "instruments": ["808 Kick", "909 Kick"]}
  ]
}
```

Groups appear in declaration order. Members use the existing alphabetical
instrument order. Each member names an immediate instrument folder or a flat
Lisp file's stem, without an extension. An instrument may belong to one group.
Unlisted instruments remain visible after the groups. The user library keeps
its physical folder structure; these files are read only in the factory tier.

Duplicate, missing or ambiguous members, empty groups, invalid labels and
unsupported versions report an error with the category file's path. Category
folders cannot receive file drops. Instrument rows keep their original load
and drag IDs. Searching a category shows all its members; searching an
instrument keeps the category above that result.

Current definitions live in `content/instruments/Drums/.categories.json` and
`content/instruments/Physical Models/.categories.json`. Factory content ships
verbatim, including these files.
