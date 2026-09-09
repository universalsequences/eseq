# Customization

Everything in the interface is Lisp under `content/ui/`, and your own
files are loaded on top of it. Rebinding a key is one line:

```lisp
(bind-key "C-c m" "eseq.manual/open-manual")
```

Any zero-argument `def` becomes an `M-x` command. For example the model
chooser is `M-x choose-model`, or [open it from here](action:eseq.choose-model/choose-model).

- `defcustom` variables are listed in the customize page (coming soon)
- Key bindings and functions get generated reference pages in a later
  release
