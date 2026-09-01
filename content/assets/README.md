# Tensor assets

ESeq tensor assets are UTF-8 JSON files used by DGenLisp `tensor @file`
references and the patcher's asset library. Put shared factory assets below this
directory; put your own shared assets below the application's user `assets/`
directory. Keep the same relative path when overriding a factory asset.

## Minimal example

Save this as `wavetables/two-waves.json`:

```json
{
  "shape": [4, 2],
  "data": [
    0.0, 1.0, 0.0, -1.0,
    -1.0, 0.0, 1.0, 0.0
  ],
  "kind": "wavetable-bank",
  "waves_per_set": 2,
  "sets": ["Example"],
  "wave_names": ["Sine-like", "Reverse sine-like"],
  "source": "Original example data; CC0"
}
```

Reference it from DGenLisp with the path relative to an asset root:

```lisp
(def bank
  (tensor @shape [4 2] @file "wavetables/two-waves.json"))
```

The `@shape` in source must match the asset's `shape`.

## Required data contract

Every asset is a JSON **object** with these keys:

- `shape`: a non-empty array of positive integers.
- `data`: a flat array of finite numbers. Its length must equal the product of
  the dimensions in `shape`.

Data is laid out in shape order. A two-dimensional wavetable bank uses
`shape: [frame_len, wave_count]` and wave-major storage:

```text
index = wave * frame_len + sample
```

A bare top-level JSON array is legacy wavetable-viewer input, not a tensor
asset. It will not appear in the patcher asset library and cannot be imported
as an asset. Use the object form for all new files. Use a `.json` extension for
assets that should appear in the library.

Consumers tolerate unknown object keys, so authors may add private metadata
without breaking compilation.

## Optional standard metadata

| Key | Type | Meaning |
| --- | --- | --- |
| `kind` | string | Asset type, such as `"wavetable-bank"`, `"impulse"`, or `"filter-table"`. Omit for a generic tensor. |
| `layout` | string | Human-readable indexing formula; documentation only. |
| `source` | string | Provenance or license note; documentation only. |
| `waves_per_set` | positive integer | Number of waves in each wavetable set. The wave count should be divisible by it. |
| `sets` | array of strings | Set display names. Its expected length is `wave_count / waves_per_set`. |
| `wave_names` | array of strings | Per-wave display names. Its expected length is the total wave count. |

For `kind: "wavetable-bank"`, keep the shape two-dimensional as
`[frame_len, wave_count]`. Sets are metadata groups over the wave axis, not a
third tensor dimension.

Metadata is advisory: inconsistent metadata does not invalidate otherwise
valid tensor data. A consumer may truncate an overlong list, clamp to the
available entries, or warn. Authors should still keep the values consistent so
selectors and inspectors have complete labels.

## Resolution order

For a relative `@file` or `@default-file` reference, ESeq uses the first file
found in this order:

1. The patch's own directory (the saved instrument/effect directory or the
   current temporary draft directory).
2. The mutable user asset library (`assets/` under application support; in a
   development checkout, `.local/assets/`).
3. The factory asset library (this `content/assets/` directory in development,
   or the bundled `Resources/assets/` directory in a packaged app).

For example, all tiers may contain `wavetables/two-waves.json`; the patch-local
copy wins, then the user copy, then the factory copy. Missing references fail
compilation and report the searched roots. Prefer portable, forward-slash
relative references that remain inside an asset root; do not couple content to
machine-specific absolute paths.

Shared-library assets are referenced in place. Saving or forking a patch does
not copy them into the patch.

## Patcher import behavior

There are two different drop operations:

- Dragging an entry from the patcher's **Assets** section inserts a tensor node
  with the asset's declared shape and its library-relative `@file` path. It
  does not copy the shared asset.
- Dropping a file from the operating system onto the patcher canvas is the
  escape hatch for a patch-specific asset. ESeq verifies that it has a
  non-empty positive `shape`, copies it into the current patch or draft's
  `waves/` directory, and inserts a tensor node using a `waves/<name>`
  reference. Saving or forking the patch carries this local file along.

An OS drop never overwrites an existing file. Identical content reuses the
existing file; different content with the same name receives a numeric suffix,
for example `bank-1.json`. If node insertion fails after creating a copy, ESeq
removes that new copy.

The patcher library scans asset roots recursively, but only `.json` files with
a non-empty positive `shape` are listed. Full `data` length and numeric validity
remain part of the contract and are enforced when the tensor is compiled.
