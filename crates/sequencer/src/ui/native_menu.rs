//! AppKit adapter for validated Lisp definitions. Menu content and policy live in Lisp.
use crate::application_menu::{MenuNode, SharedMenuState, Shortcut, SOURCE};
use crate::*;
use muda::accelerator::{Key, KeyAccelerator, Modifiers};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver};

pub(crate) fn accelerator(shortcut: &Shortcut) -> Result<KeyAccelerator, String> {
    let mut modifiers = Modifiers::empty();
    for modifier in &shortcut.modifiers {
        modifiers |= match modifier.as_str() {
            "primary" | "command" | "super" => Modifiers::SUPER,
            "control" => Modifiers::CONTROL,
            "alt" => Modifiers::ALT,
            "shift" => Modifiers::SHIFT,
            _ => return Err(format!("unknown shortcut modifier: {modifier}")),
        };
    }
    let key = Key::from_str(&shortcut.key).map_err(|e| format!("invalid shortcut key: {e:?}"))?;
    Ok(KeyAccelerator::new(Some(modifiers), key))
}

enum NativeItem {
    Command(MenuItem),
    Check(CheckMenuItem),
    Submenu(Submenu),
}
struct ItemBinding {
    id: String,
    item: NativeItem,
}

pub(crate) struct NativeMenu {
    root: Option<Menu>,
    bindings: Vec<ItemBinding>,
    applied: Vec<Option<MenuNode>>,
    state: SharedMenuState,
    attempted_revision: u64,
    events: Receiver<MenuEvent>,
}

fn append_nodes(
    nodes: &[Option<MenuNode>],
    append: &mut dyn FnMut(&dyn muda::IsMenuItem) -> muda::Result<()>,
    bindings: &mut Vec<ItemBinding>,
) -> Result<(), String> {
    for node in nodes {
        let Some(node) = node else {
            append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
            continue;
        };
        if let Some(children) = &node.children {
            let submenu = Submenu::new(&node.label, node.enabled);
            append_nodes(children, &mut |item| submenu.append(item), bindings)?;
            append(&submenu).map_err(|e| e.to_string())?;
            bindings.push(ItemBinding {
                id: node.id.clone(),
                item: NativeItem::Submenu(submenu),
            });
        } else if node.role.as_deref().is_some_and(|role| role != "quit") {
            let text = Some(node.label.as_str());
            let item = match node.role.as_deref() {
                Some("services") => PredefinedMenuItem::services(text),
                Some("hide") => PredefinedMenuItem::hide(text),
                Some("hide-others") => PredefinedMenuItem::hide_others(text),
                Some("show-all") => PredefinedMenuItem::show_all(text),
                _ => unreachable!("roles were validated before installation"),
            };
            append(&item).map_err(|e| e.to_string())?;
        } else if let Some(checked) = node.checked {
            let item = CheckMenuItem::new(&node.label, node.enabled, checked, None);
            if let Some(shortcut) = &node.shortcut {
                item.set_key_accelerator(Some(accelerator(shortcut)?))
                    .map_err(|e| e.to_string())?;
            }
            append(&item).map_err(|e| e.to_string())?;
            bindings.push(ItemBinding {
                id: node.id.clone(),
                item: NativeItem::Check(item),
            });
        } else {
            // OS event ids are per installation, so queued events from a
            // removed menu cannot accidentally invoke a replacement item.
            let item = MenuItem::new(&node.label, node.enabled, None);
            if let Some(shortcut) = &node.shortcut {
                item.set_key_accelerator(Some(accelerator(shortcut)?))
                    .map_err(|e| e.to_string())?;
            }
            append(&item).map_err(|e| e.to_string())?;
            bindings.push(ItemBinding {
                id: node.id.clone(),
                item: NativeItem::Command(item),
            });
        }
    }
    Ok(())
}

fn find_node<'a>(
    nodes: &'a [Option<MenuNode>],
    id: &str,
    parent_enabled: bool,
) -> Option<(&'a MenuNode, bool)> {
    for node in nodes.iter().flatten() {
        let enabled = parent_enabled && node.enabled;
        if node.id == id {
            return Some((node, enabled));
        }
        if let Some(children) = &node.children {
            if let Some(found) = find_node(children, id, enabled) {
                return Some(found);
            }
        }
    }
    None
}

impl NativeMenu {
    /// Main thread only, after the application event loop exists.
    pub(crate) fn attach(
        state: SharedMenuState,
        editor: &mut Editor,
        backend: &AppBackend,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let waker = backend.event_loop_waker();
        MenuEvent::set_event_handler(Some(move |event| {
            if sender.send(event).is_ok() {
                if let Some(waker) = &waker {
                    waker.wake();
                }
            }
        }));
        state.borrow_mut().supported = true;
        if let Err(error) =
            editor
                .runtime_mut()
                .invalidate_reactive_source(SOURCE, "supported", Value::Bool(true))
        {
            editor.handle_host_event(HostEvent::Error(format!(
                "Menu capability update failed: {error:?}"
            )));
        }
        Self {
            root: None,
            bindings: Vec::new(),
            applied: Vec::new(),
            state,
            attempted_revision: 0,
            events,
        }
    }

    pub(crate) fn sync(&mut self, editor: &mut Editor) {
        let (revision, nodes) = {
            let state = self.state.borrow();
            if state.revision == self.attempted_revision {
                return;
            }
            (state.revision, state.nodes.clone())
        };
        self.attempted_revision = revision;
        if application_menu::same_structure(&self.applied, &nodes) {
            // A reactive enabling change or a new callback does not rebuild
            // NSMenus, disturb an open menu, or leave stale callbacks behind.
            for binding in &self.bindings {
                if let Some((node, _)) = find_node(&nodes, &binding.id, true) {
                    match &binding.item {
                        NativeItem::Command(item) => item.set_enabled(node.enabled),
                        NativeItem::Check(item) => {
                            item.set_enabled(node.enabled);
                            item.set_checked(node.checked.unwrap_or(false));
                        }
                        NativeItem::Submenu(item) => item.set_enabled(node.enabled),
                    }
                }
            }
        } else {
            let replacement = if nodes.is_empty() {
                None
            } else {
                Some(Menu::new())
            };
            let mut bindings = Vec::new();
            if let Some(root) = &replacement {
                if let Err(error) =
                    append_nodes(&nodes, &mut |item| root.append(item), &mut bindings)
                {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Native menu update failed: {error}"
                    )));
                    return;
                }
            }
            if let Some(previous) = self.root.take() {
                previous.remove_for_nsapp();
            }
            if let Some(root) = &replacement {
                root.init_for_nsapp();
            }
            self.root = replacement;
            self.bindings = bindings;
        }
        self.applied = nodes;
        let installed = self.root.is_some();
        if self.state.borrow().installed != installed {
            self.state.borrow_mut().installed = installed;
            if let Err(error) = editor.runtime_mut().invalidate_reactive_source(
                SOURCE,
                "installed",
                Value::Bool(installed),
            ) {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Menu availability update failed: {error:?}"
                )));
            }
        }
        editor.refresh_runtime_side_effects();
    }

    pub(crate) fn drain(&self, editor: &mut Editor) {
        while let Ok(event) = self.events.try_recv() {
            let Some(binding) = self.bindings.iter().find(|binding| {
                matches!(&binding.item,
                NativeItem::Command(item) if item.id() == &event.id)
                    || matches!(&binding.item, NativeItem::Check(item) if item.id() == &event.id)
            }) else {
                continue;
            };
            let Some((node, true)) = find_node(&self.applied, &binding.id, true) else {
                continue;
            };
            if let NativeItem::Check(item) = &binding.item {
                // Lisp is authoritative, even when a callback declines or fails.
                item.set_checked(node.checked.unwrap_or(false));
            }
            application_menu::invoke_action(node, editor);
        }
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        if let Some(root) = &self.root {
            root.remove_for_nsapp();
        }
        let mut state = self.state.borrow_mut();
        state.installed = false;
        state.supported = false;
    }
}
