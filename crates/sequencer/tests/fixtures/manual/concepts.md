# Reading this manual

eseq is built from **buffers** shown in **tiles**. Every panel you see, from
the step grid to this page, is a buffer. Buffers are switched with `C-x b`
and commands run from the `M-x` prompt.

## Navigating the manual

The manual is a graph of pages linked by menus. Inside the `*manual*`
buffer:

- `n` and `p` move to the next and previous page in the parent menu
- `u` goes up to the parent page
- `l` goes back to the last page you were on
- `t` returns to the [top page](index)
- `q` closes the manual and restores your previous buffer

Links are clickable. Some links run a command in the app rather than
opening a page, for example [open the mixer](action:switch-to-buffer "*mixer*")
(the same as `C-x b *mixer*`). On the website those links are plain text.

## Where things live

Project source is on [GitHub](https://github.com/universalsequences/eseq).
The manual is written in a small markdown subset described in
`docs/manual-spec.md`.
