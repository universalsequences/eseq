//! `describe-function`: the `*Help*` view of a symbol's signature and docs,
//! from a Lisp definition header (docs/command-palette-spec.md §3.3) or a
//! documented native.

use super::{Editor, MinibufferMode, key_str};
use crossterm::event::{KeyEvent, KeyModifiers};
use crate::mode::BufferMode;

pub(super) const HELP_BUFFER: &str = "*Help*";

impl Editor {
    pub(super) fn start_describe_function(&mut self) {
        self.completion = None;
        self.minibuffer = None;
        let candidates = self.collect_mx_candidates();
        self.minibuffer_input =
            Some(MinibufferMode::DescribeFunction { input: String::new(), candidates, selected: 0 });
    }

    /// `describe-key`: the next key (a whole chord, if it starts one) is
    /// named, with what it runs here, instead of running.
    pub(super) fn start_describe_key(&mut self) {
        self.completion = None;
        self.minibuffer_input = None;
        self.describing_key = Some(Vec::new());
        self.show_sticky_message("Describe key: ");
    }

    pub(super) fn describe_key_press(&mut self, key: KeyEvent) {
        let mut chord = self.describing_key.take().unwrap_or_default();
        chord.push(key_str(key));
        let spelled = chord.join(" ");
        // Dispatch opens a chord for a modified global prefix before the
        // mode keymap is consulted, so such a key waits for the rest of the
        // chord whatever the mode binds; other prefixes wait only when no
        // binding claims the key itself.
        let opens_chord = if chord.len() == 1 {
            self.opens_chord_before_mode(key)
                || (self.binding_has_prefix(&spelled) && self.key_binding_of(&spelled).is_none())
        } else {
            false
        };
        if opens_chord {
            self.show_sticky_message(format!("Describe key: {spelled} -"));
            self.describing_key = Some(chord);
            return;
        }
        let message = self.describe_key_text(&spelled, (chord.len() == 1).then_some(key));
        self.show_sticky_message(message);
    }

    /// Whether `key` opens a chord before the mode's `:on-key` or keymap
    /// sees it, as `handle_key` does: a modified key that is a prefix of
    /// some binding but not itself a direct global binding.
    fn opens_chord_before_mode(&self, key: KeyEvent) -> bool {
        let spelled = super::key_str(key);
        key.modifiers != KeyModifiers::NONE
            && !self.lisp_bindings.contains_key(&spelled)
            && self.binding_has_prefix(&spelled)
    }

    /// What `spelled` (a `key_str` spelling, or a chord of them) runs in the
    /// active buffer: the mode keymap (nearest mode first), else the global
    /// map, else a builtin. Earlier layers that see a single key first are
    /// named too: a focused widget, the mode's `:on-key` handler. The rest
    /// of a chord never reaches `:on-key`.
    pub fn describe_key_text(&self, spelled: &str, key: Option<KeyEvent>) -> String {
        let runs = self
            .key_binding_of(spelled)
            .or_else(|| key.and_then(|key| self.builtins.get(&key).map(|cmd| (cmd.clone(), "built-in".to_string()))));
        let mut text = match runs {
            Some((handler, source)) => format!("{spelled} runs {handler} ({source})"),
            None => format!("{spelled} is undefined here"),
        };
        let reaches_on_key = key.is_some_and(|key| !self.opens_chord_before_mode(key));
        if let Some(on_key) = self.active_mode_on_key().filter(|_| reaches_on_key) {
            text.push_str(&format!("; the mode's :on-key {on_key} sees it first"));
        }
        if self.active_leaf().focused_widget_id.is_some() {
            text.push_str("; a focused widget sees it before that");
        }
        text
    }

    /// The handler `spelled` is bound to in the mode chain or the global
    /// map, and where.
    fn key_binding_of(&self, spelled: &str) -> Option<(String, String)> {
        self.active_mode_chain()
            .into_iter()
            .find_map(|mode| {
                mode.keybindings
                    .get(spelled)
                    .map(|handler| (handler.clone(), format!("{} keymap", mode.name)))
            })
            .or_else(|| {
                self.lisp_bindings
                    .get(spelled)
                    .map(|handler| (handler.clone(), "global keymap".to_string()))
            })
    }

    /// Shows `name`'s help in the `*Help*` buffer, replacing what it held.
    pub fn describe_function(&mut self, name: &str) {
        let text = self.describe_function_text(name);
        let id = self.upsert_read_only_scratch_buffer_with_mode(HELP_BUFFER, &text, BufferMode::ESeqLisp);
        if let Some(buffer) = self.buffers.iter_mut().find(|buffer| buffer.id == id) {
            buffer.cursor = (0, 0);
            buffer.scroll_top = 0;
        }
        self.set_active_buffer(id);
    }

    pub fn describe_function_text(&mut self, name: &str) -> String {
        if let Some(header) = self.runtime.definition_header(name) {
            let mut text = format!("{}\n\n", header.signature);
            text.push_str(header.doc.as_deref().unwrap_or("Not documented."));
            text.push('\n');
            if let Some(title) = &header.command {
                text.push_str(&format!("\nCommand: {title}\n"));
                for step in 0..header.choices.len() {
                    let prompt = header.step_prompt(step).unwrap_or_else(|| "Value".to_string());
                    text.push_str(&format!("Choice: {prompt}\n"));
                }
            }
            if let Some(group) = &header.group {
                text.push_str(&format!("Group: {group}\n"));
            }
            return text;
        }
        match self.runtime.completion_metadata_for(name) {
            Some(metadata) => {
                let mut text = format!("{}\n\n{}\n", metadata.signature, metadata.docs);
                if crate::widgets::BUILTIN_WIDGET_NAMES.contains(&name) {
                    if !metadata.keyword_args.is_empty() {
                        text.push_str(&format!("\nProps: {}\n", metadata.keyword_args.join(" ")));
                    }
                    text.push_str(
                        "Every widget also takes :key (a stable identity), :width and :height (cells, or :fill) and :flex N (a share of a stack's leftover space).\n",
                    );
                }
                text
            }
            None => format!("{name}\n\nNot documented.\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Editor, EditorConfig};
    use crate::runtime::Runtime;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A mode binding for a modified global chord prefix never runs, since
    /// dispatch opens the chord first; describe-key follows the chord too.
    #[test]
    fn describe_key_follows_a_global_chord_prefix_the_mode_also_binds() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*prefixed*", "");
        editor
            .runtime_mut()
            .eval_str(
                r#"
(def mode-cx () (host-command "mode-cx-ran" true))
(def other-window-cmd () (host-command "other-ran" true))
(def watch-keys (k txt) false)
(bind-key "C-x o" "other-window-cmd")
(define-mode "prefixed-mode" :on-key "watch-keys")
(mode-bind-key "prefixed-mode" "C-x" "mode-cx")
(set-buffer-mode-for "*prefixed*" "prefixed-mode")
"#,
            )
            .expect("install bindings");
        editor.refresh_runtime_side_effects();
        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        let o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);

        editor.run_command("describe-key");
        editor.handle_key(ctrl_x);
        assert_eq!(editor.minibuffer.clone().unwrap_or_default(), "Describe key: C-x -");
        editor.handle_key(o);
        assert_eq!(
            editor.minibuffer.clone().unwrap_or_default(),
            "C-x o runs other-window-cmd (global keymap)"
        );
    }
}
