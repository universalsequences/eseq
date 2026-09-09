//! Platform-independent Lisp menu definitions. No application menu policy lives here.
use crate::*;

pub(crate) const SOURCE: &str = "__native-menu";
pub(crate) type SharedMenuState = Rc<RefCell<MenuState>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub key: String,
    pub modifiers: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct MenuNode {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub checked: Option<bool>,
    pub native_only: bool,
    pub shortcut: Option<Shortcut>,
    pub action: Option<Value>,
    pub role: Option<String>,
    pub children: Option<Vec<Option<MenuNode>>>,
}

impl MenuNode {
    pub fn same_structure(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.shortcut == other.shortcut
            && self.role == other.role
            && self.checked.is_some() == other.checked.is_some()
            && match (&self.children, &other.children) {
                (None, None) => true,
                (Some(a), Some(b)) => same_structure(a, b),
                _ => false,
            }
    }

    fn value(&self) -> Value {
        let mut fields = vec![
            ("id", Value::String(self.id.clone())),
            ("label", Value::String(self.label.clone())),
            ("enabled", Value::Bool(self.enabled)),
            ("native-only", Value::Bool(self.native_only)),
        ];
        if let Some(checked) = self.checked {
            fields.push(("checked", Value::Bool(checked)));
        }
        if let Some(shortcut) = &self.shortcut {
            fields.push(("shortcut", shortcut_value(shortcut)));
        }
        if let Some(action) = &self.action {
            fields.push(("on-select", action.clone()));
        }
        if let Some(role) = &self.role {
            fields.push(("role", Value::Keyword(role.clone())));
        }
        if let Some(children) = &self.children {
            fields.push(("items", nodes_value(children)));
        }
        map_value(fields)
    }
}

pub(crate) fn same_structure(a: &[Option<MenuNode>], b: &[Option<MenuNode>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_structure(b),
            _ => false,
        })
}

fn nodes_value(nodes: &[Option<MenuNode>]) -> Value {
    list_value(
        nodes
            .iter()
            .map(|node| node.as_ref().map(MenuNode::value).unwrap_or(Value::Nil)),
    )
}

fn shortcut_value(shortcut: &Shortcut) -> Value {
    map_value([
        ("key", Value::String(shortcut.key.clone())),
        (
            "modifiers",
            list_value(shortcut.modifiers.iter().cloned().map(Value::Keyword)),
        ),
    ])
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputContext {
    pub blocked: bool,
    pub text_input: bool,
    pub ui_view: bool,
}

#[derive(Default)]
pub(crate) struct MenuState {
    pub revision: u64,
    pub nodes: Vec<Option<MenuNode>>,
    pub supported: bool,
    pub installed: bool,
    pub context: InputContext,
}

fn boolean(
    map: &HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match map.get(key).map(|v| v.borrow().clone()) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(value),
        _ => Err(format!("menu :{key} must be a boolean")),
    }
}

fn required_string(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Result<String, String> {
    map_string(map, key)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("menu requires nonempty :{key}"))
}

fn parse_shortcut(value: &Value) -> Result<Shortcut, String> {
    let Value::Map(map) = value else {
        return Err("menu :shortcut must be a map".into());
    };
    let key = required_string(map, "key")?;
    let mut modifiers = Vec::new();
    if let Some(value) = map.get("modifiers") {
        let Value::List(items) = &*value.borrow() else {
            return Err("shortcut :modifiers must be a list".into());
        };
        for item in items {
            let name = match &*item.borrow() {
                Value::Keyword(s) | Value::String(s) => s.clone(),
                _ => return Err("shortcut modifier must be a keyword".into()),
            };
            if !["primary", "command", "control", "shift", "alt", "super"].contains(&name.as_str())
            {
                return Err(format!("unknown shortcut modifier :{name}"));
            }
            if !modifiers.contains(&name) {
                modifiers.push(name);
            }
        }
    }
    let shortcut = Shortcut { key, modifiers };
    #[cfg(target_os = "macos")]
    crate::native_menu::accelerator(&shortcut)?;
    Ok(shortcut)
}

fn parse_nodes(
    value: &Value,
    ids: &mut HashSet<String>,
    depth: usize,
) -> Result<Vec<Option<MenuNode>>, String> {
    if depth > 16 {
        return Err("menus may nest at most 16 levels".into());
    }
    let Value::List(items) = value else {
        return Err("menu definition and :items must be lists".into());
    };
    let mut result = Vec::new();
    for item in items {
        let item = item.borrow();
        if matches!(*item, Value::Nil) && depth > 0 {
            result.push(None);
            continue;
        }
        let Value::Map(map) = &*item else {
            return Err("menu entries must be maps (or nil separators inside menus)".into());
        };
        let id = required_string(map, "id")?;
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate menu id: {id}"));
        }
        let label = required_string(map, "label")?;
        let children = map
            .get("items")
            .map(|v| parse_nodes(&v.borrow(), ids, depth + 1))
            .transpose()?;
        if depth == 0 && children.is_none() {
            return Err("top-level menu entries require :items".into());
        }
        let shortcut = map
            .get("shortcut")
            .map(|v| parse_shortcut(&v.borrow()))
            .transpose()?;
        let role = map_string(map, "role");
        if let Some(role) = &role {
            if !["services", "hide", "hide-others", "show-all", "quit"].contains(&role.as_str()) {
                return Err(format!("unknown native menu role: {role}"));
            }
        }
        let action = map.get("on-select").map(|v| v.borrow().clone());
        if let Some(action) = &action {
            if !matches!(
                action,
                Value::String(_)
                    | Value::Symbol(_)
                    | Value::Function(_)
                    | Value::Closure(_, _)
                    | Value::NativeFunction(_)
                    | Value::OverrideDispatcher(_)
                    | Value::OverrideOriginal(_)
            ) {
                return Err(format!(
                    "menu {id}: :on-select must be a function or global function name"
                ));
            }
        }
        if children.is_some() && (action.is_some() || role.is_some() || shortcut.is_some()) {
            return Err(format!(
                "submenu {id} cannot have :on-select, :role or :shortcut"
            ));
        }
        if children.is_none() && (action.is_some() == role.is_some()) {
            return Err(format!(
                "menu item {id} needs exactly one of :on-select or :role"
            ));
        }
        if role.as_deref().is_some_and(|role| role != "quit") && shortcut.is_some() {
            return Err(format!(
                "system role {id} uses the operating system's shortcut"
            ));
        }
        if role.as_deref().is_some_and(|role| role != "quit") && !boolean(map, "enabled", true)? {
            return Err(format!("system role {id} is enabled by the operating system; disable its parent menu instead"));
        }
        if map.contains_key("checked") && (children.is_some() || role.is_some()) {
            return Err(format!(
                "menu {id}: :checked requires an ordinary action item"
            ));
        }
        result.push(Some(MenuNode {
            id,
            label,
            children,
            shortcut,
            role,
            action,
            enabled: boolean(map, "enabled", true)?,
            native_only: boolean(map, "native-only", false)?,
            checked: if map.contains_key("checked") {
                Some(boolean(map, "checked", false)?)
            } else {
                None
            },
        }));
    }
    Ok(result)
}

pub(crate) fn register_natives(runtime: &mut Runtime) -> SharedMenuState {
    runtime.register_native("seq-recent-projects", |_, _| {
        let names = sequencer::recent_projects::list().map_err(|e| e.to_string())?;
        Ok(list_value(
            names
                .into_iter()
                .filter(|name| {
                    sequencer::app_paths::app_paths()
                        .projects_dir()
                        .join(format!("{name}.json"))
                        .is_file()
                })
                .map(Value::String),
        ))
    });
    let state = Rc::new(RefCell::new(MenuState::default()));
    runtime.register_native_with_docs("native-menu-validate", "(native-menu-validate menus)",
        "Validate a menu tree without changing the configured menus. Returns true, or false with a diagnostic.", |args, _| {
            if args.len() != 1 { return Err("native-menu-validate expects one menu list".into()); }
            parse_nodes(&args[0], &mut HashSet::new(), 0)?;
            Ok(Value::Bool(true))
        });
    runtime.register_native_with_docs("native-menu-activate", "(native-menu-activate id)",
        "Queue a configured callback/quit action by item id, respecting its enabled state and ancestor menus.", |args, ctx| {
            let Some(Value::String(id)) = args.first() else { return Err("expected menu item id string".into()); };
            ctx.enqueue_command(HostCommand::Custom { name: "native-menu-activate".into(), payload: Value::String(id.clone()) });
            Ok(Value::Bool(true))
        });
    let shared = state.clone();
    runtime.register_native_with_docs("native-menu-set!", "(native-menu-set! menus)",
        "Atomically replace the desired menu tree. Menus have :id :label :items; items have :on-select or :role, optional :enabled and :shortcut. nil separates items. Available in headless runtimes for validation/fallback rendering.", move |args, ctx| {
            if args.len() != 1 { return Err("native-menu-set! expects one menu list".into()); }
            let nodes = parse_nodes(&args[0], &mut HashSet::new(), 0)?;
            let revision = {
                let mut state = shared.borrow_mut();
                state.nodes = nodes;
                state.revision += 1;
                state.revision
            };
            ctx.invalidate_reactive_source(SOURCE, "definition", Value::Number(revision as f64));
            Ok(Value::Bool(true))
        });
    let shared = state.clone();
    runtime.register_native_with_docs("native-menu-definition", "(native-menu-definition)",
        "Return an owned snapshot of the configured menu tree. Reactive readers update when the definition changes.", move |_, ctx| {
            ctx.track_reactive_read(SOURCE, "definition");
            Ok(nodes_value(&shared.borrow().nodes))
        });
    for (name, field) in [
        ("native-menu-supported?", "supported"),
        ("native-menu-installed?", "installed"),
    ] {
        let shared = state.clone();
        runtime.register_native_with_docs(name, format!("({name})"),
            "Report whether this runtime has an attached native menu backend / an installed native menu. False in headless captures and on unsupported platforms.", move |_, ctx| {
                ctx.track_reactive_read(SOURCE, field);
                let state = shared.borrow();
                Ok(Value::Bool(if field == "supported" { state.supported } else { state.installed }))
            });
    }
    let shared = state.clone();
    runtime.register_native_with_docs("native-menu-context", "(native-menu-context)",
        "Reactive input context map: :blocked (modal/prompt), :text-input, :ui-view. Menu enabling policy belongs in Lisp.", move |_, ctx| {
            ctx.track_reactive_read(SOURCE, "context");
            let input = shared.borrow().context;
            Ok(map_value([("blocked", Value::Bool(input.blocked)), ("text-input", Value::Bool(input.text_input)), ("ui-view", Value::Bool(input.ui_view))]))
        });
    runtime.register_native_with_docs("native-menu-shortcut-label", "(native-menu-shortcut-label shortcut)",
        "Format a shortcut map (:key string :modifiers list) using platform key labels. Modifiers: :primary :command :control :shift :alt :super.", |args, _| {
            let value = args.first().ok_or("expected shortcut map")?;
            let shortcut = parse_shortcut(value)?;
            Ok(Value::String(shortcut_label(&shortcut)))
        });
    state
}

pub(crate) fn sync_context(state: &SharedMenuState, editor: &mut Editor) {
    let context = InputContext {
        blocked: editor.modal_is_open()
            || editor.minibuffer_prompt().is_some()
            || editor.prompt_text().is_some(),
        text_input: focused_widget_captures_text_input(editor),
        ui_view: editor.active_buffer().view_mode == ViewMode::UiOnly,
    };
    if state.borrow().context == context {
        return;
    }
    state.borrow_mut().context = context;
    let generation = (context.blocked as u8)
        | ((context.text_input as u8) << 1)
        | ((context.ui_view as u8) << 2);
    if let Err(error) = editor.runtime_mut().invalidate_reactive_source(
        SOURCE,
        "context",
        Value::Number(generation as f64),
    ) {
        editor.handle_host_event(HostEvent::Error(format!(
            "Menu context update failed: {error:?}"
        )));
    }
}

/// Shared callback delivery for toolbar actions. Native actions use the applied
/// tree, so failed or removed installations never deliver unrelated callbacks.
pub(crate) fn activate(state: &SharedMenuState, editor: &mut Editor, id: &str) {
    fn find(nodes: &[Option<MenuNode>], id: &str, enabled: bool) -> Option<MenuNode> {
        for node in nodes.iter().flatten() {
            let enabled = enabled && node.enabled;
            if node.id == id {
                return enabled.then(|| node.clone());
            }
            if let Some(children) = &node.children {
                if let Some(found) = find(children, id, enabled) {
                    return Some(found);
                }
            }
        }
        None
    }
    let node = find(&state.borrow().nodes, id, true);
    if let Some(node) = node {
        invoke_action(&node, editor);
    }
}

pub(crate) fn invoke_action(node: &MenuNode, editor: &mut Editor) {
    if node.role.as_deref() == Some("quit") {
        editor.request_quit();
        return;
    }
    let Some(action) = &node.action else {
        return;
    };
    let result = match action {
        Value::String(name) | Value::Symbol(name) => {
            editor.runtime_mut().invoke_global(name, vec![])
        }
        callback => editor.runtime_mut().invoke(callback.clone(), vec![]),
    };
    if let Err(error) = result {
        editor.handle_host_event(HostEvent::Error(format!(
            "Menu callback {} failed: {error:?}",
            node.id
        )));
    }
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lisp_registry_reconfigures_callbacks_and_reactive_enablement() {
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let state = register_natives(editor.runtime_mut());
        editor
            .runtime_mut()
            .eval_str(include_str!("../../../../content/ui/menus.lisp"))
            .unwrap();
        editor
            .runtime_mut()
            .eval_str(
                r#"
            (module menu-test)
            (defstate count 0)
            (eseq.menus/register-menu
              (dict :id "tools" :label "Tools"
                :enabled-when (lambda () (not (get (native-menu-context) :blocked)))
                :items (list
                  (dict :id "tools-run" :label "Run"
                    :shortcut (dict :key "r" :modifiers (list :primary :shift))
                    :on-select (lambda () (set! count (+ count 1)))))))
        "#,
            )
            .unwrap();
        assert_eq!(state.borrow().nodes.len(), 1);
        activate(&state, &mut editor, "tools-run");
        assert!(matches!(
            editor.runtime_mut().eval_str("menu-test/count").unwrap(),
            Some(Value::Number(1.0))
        ));
        state.borrow_mut().context.blocked = true;
        editor
            .runtime_mut()
            .invalidate_reactive_source(SOURCE, "context", Value::Number(1.0))
            .unwrap();
        assert!(!state.borrow().nodes[0].as_ref().unwrap().enabled);
        activate(&state, &mut editor, "tools-run");
        assert!(matches!(
            editor.runtime_mut().eval_str("menu-test/count").unwrap(),
            Some(Value::Number(1.0))
        ));
        editor
            .runtime_mut()
            .eval_str(
                r#"
            (eseq.menus/register-menu (dict :id "tools" :label "Tools"
              :items (list (dict :id "tools-run" :label "Run"
                :on-select (lambda () (set! menu-test/count 42))))))
        "#,
            )
            .unwrap();
        assert_eq!(state.borrow().nodes.len(), 1);
        activate(&state, &mut editor, "tools-run");
        assert!(matches!(
            editor.runtime_mut().eval_str("menu-test/count").unwrap(),
            Some(Value::Number(42.0))
        ));
        editor
            .runtime_mut()
            .eval_str("(eseq.menus/remove-menu \"tools\")")
            .unwrap();
        assert!(state.borrow().nodes.is_empty());
    }

    #[test]
    fn invalid_menu_replacement_preserves_previous_definition() {
        let mut runtime = Runtime::new();
        let state = register_natives(&mut runtime);
        runtime
            .eval_str(
                r#"(native-menu-set! (list (dict :id "valid" :label "Valid" :items (list))))"#,
            )
            .unwrap();
        let revision = state.borrow().revision;
        for source in [
            r#"(native-menu-set! (list (dict :id "same" :label "A" :items (list)) (dict :id "same" :label "B" :items (list))))"#,
            r#"(native-menu-set! (list (dict :id "root" :label "Root" :items (list (dict :id "bad" :label "Bad" :on-select 123)))))"#,
            r#"(native-menu-set! (list (dict :id "root" :label "Root" :items (list (dict :id "bad" :label "Bad" :role :unknown)))))"#,
            r#"(native-menu-set! (list (dict :id "root" :label "Root" :items (list (dict :id "bad" :label "Bad" :on-select (lambda () nil) :shortcut (dict :key "a" :modifiers (list :invalid)))))))"#,
        ] {
            assert!(
                matches!(runtime.eval_str(source).unwrap(), Some(Value::Bool(false))),
                "{source}"
            );
            assert_eq!(state.borrow().revision, revision);
            assert_eq!(state.borrow().nodes[0].as_ref().unwrap().id, "valid");
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn choose_sample_paths() -> Result<Vec<PathBuf>, String> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::MainThreadMarker;
    let mtm = MainThreadMarker::new().ok_or("File picker requires the UI thread")?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(true);
    panel.setAllowsMultipleSelection(true);
    if panel.runModal() != NSModalResponseOK {
        return Ok(vec![]);
    }
    Ok(panel
        .URLs()
        .iter()
        .filter_map(|url| url.path().map(|path| PathBuf::from(path.to_string())))
        .collect())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn choose_sample_paths() -> Result<Vec<PathBuf>, String> {
    Err("Use file-manager drag and drop to import samples on this platform".into())
}

pub(crate) fn search_commands(editor: &mut Editor) -> Result<(), String> {
    let value = editor
        .runtime_mut()
        .invoke_global("native-menu-definition", vec![])
        .map_err(|e| format!("{e:?}"))?
        .ok_or("Menu definition is unavailable")?;
    let nodes = parse_nodes(&value, &mut HashSet::new(), 0)?;
    fn collect(nodes: &[Option<MenuNode>], prefix: &str, entries: &mut Vec<(String, HostCommand)>) {
        for node in nodes
            .iter()
            .flatten()
            .filter(|node| node.enabled && !node.native_only)
        {
            let label = if prefix.is_empty() {
                node.label.clone()
            } else {
                format!("{prefix} > {}", node.label)
            };
            if let Some(children) = &node.children {
                collect(children, &label, entries);
            } else if node.action.is_some() {
                let label = if let Some(key) = &node.shortcut {
                    format!("{label} ({})", shortcut_label(key))
                } else {
                    label
                };
                entries.push((
                    label,
                    HostCommand::Custom {
                        name: "native-menu-activate".into(),
                        payload: Value::String(node.id.clone()),
                    },
                ));
            }
        }
    }
    let mut entries = Vec::new();
    collect(&nodes, "", &mut entries);
    editor.open_command_choices("Search Commands".into(), entries);
    Ok(())
}

fn shortcut_label(shortcut: &Shortcut) -> String {
    let mut label = String::new();
    for modifier in &shortcut.modifiers {
        label.push_str(match (cfg!(target_os = "macos"), modifier.as_str()) {
            (true, "primary" | "command" | "super") => "⌘",
            (true, "control") => "⌃",
            (true, "shift") => "⇧",
            (true, "alt") => "⌥",
            (false, "primary" | "control") => "Ctrl+",
            (false, "shift") => "Shift+",
            (false, "alt") => "Alt+",
            _ => "Super+",
        });
    }
    label.push_str(&shortcut.key.to_uppercase());
    label
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn search_uses_menu_registry_and_restores_context_before_activation() {
        let mut editor = Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let state = register_natives(editor.runtime_mut());
        editor
            .runtime_mut()
            .eval_str(include_str!("../../../../content/ui/menus.lisp"))
            .unwrap();
        editor
            .runtime_mut()
            .eval_str(
                r#"
            (module search-test)
            (defstate count 0)
            (eseq.menus/register-menu (dict :id "tools" :label "Tools"
              :enabled-when (lambda () (not (get (native-menu-context) :blocked)))
              :items (list (dict :id "run" :label "Run"
                :on-select (lambda () (set! count (+ count 1)))))))
        "#,
            )
            .unwrap();
        search_commands(&mut editor).unwrap();
        assert!(editor.minibuffer_prompt().unwrap().contains("Tools > Run"));
        sync_context(&state, &mut editor);
        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        sync_context(&state, &mut editor);
        let commands = editor.drain_host_commands();
        assert!(commands.iter().any(|cmd| matches!(cmd, HostCommand::Custom { name, payload: Value::String(id) } if name == "native-menu-activate" && id == "run")));
        activate(&state, &mut editor, "run");
        assert!(matches!(
            editor.runtime_mut().eval_str("search-test/count").unwrap(),
            Some(Value::Number(1.0))
        ));
    }
}
