# eseqlisp

An embeddable terminal Lisp editor/runtime intended for host applications.

Current use:

- embedded inside `tinyseq` as the control Lisp
- used for scratch scripting, hook registration, and in-app instrument/effect editing

Related project:

- `tinyseq`: `/Users/alecresende/code/learning/anthropic/sequencer`
- GitHub: https://github.com/universalsequences/eseqlisp

Conceptually:

- `eseqlisp` handles buffers, keybindings, eval, host commands, and editor UI
- the host application provides builtins/native functions and owns slow async work

See also:

- [EMBEDDING.md](/Users/alecresende/code/learning/anthropic/eseqlisp/EMBEDDING.md)
- [PHASE1_EMBEDDING_SPEC.md](/Users/alecresende/code/learning/anthropic/eseqlisp/PHASE1_EMBEDDING_SPEC.md)
