# Embedding `eseqlisp`

## Minimal shape

```rust
use eseqlisp::{Editor, EditorConfig, HostEvent, Runtime};

let init_src = std::fs::read_to_string("init.lisp").unwrap_or_default();
let mut runtime = Runtime::with_init_source(init_src);

runtime.register_native("seq-toggle-step", |args, _ctx| {
    // mutate host state here
    Ok(eseqlisp::vm::Value::Bool(true))
});

let mut editor = Editor::new(runtime, EditorConfig::default());
editor.open_or_create_file_buffer("instruments/demo.lisp", "")?;

// host loop owns command execution
for command in editor.drain_host_commands() {
    // compile, save, ask agent, etc
}

editor.handle_host_event(HostEvent::Status("Compiling...".to_string()));
```

## Model

- register cheap synchronous host operations as natives
- send expensive work through `HostCommand`
- feed completion and status back through `HostEvent`
- keep DSP compilation and graph mutation in the host app
