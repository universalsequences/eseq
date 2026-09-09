# Application menus in Lisp

The factory configuration lives in `content/ui/application-menus.lisp`. It defines
all top-level menus, items, ordering, separators, native roles, shortcuts,
callbacks, and enabled-state rules. Rust only validates definitions, maintains
AppKit objects, publishes input context, and delivers callbacks on the UI thread.

Use the public `eseq.menus` module from user init or a package. Changes apply on
the next UI loop pass; no Rust rebuild or app restart is needed. The same resolved
configuration drives the toolbar when native menus are unavailable. Headless
captures keep the toolbar and do not install a process-global menu bar.

```lisp
(import eseq.menus :as menus)

(menus/register-menu
  (dict :id "tools" :label "Tools"
    :items (list
      (dict :id "tools-instrument" :label "Create Instrument…"
        :shortcut (dict :key "i" :modifiers (list :primary :shift))
        :on-select (lambda ()
          (host-command "enter-new-instrument-editor" (dict))))
      nil
      (dict :id "tools-help" :label "Help"
        :on-select (lambda () (host-command "open-help" (dict)))))))
```

`register-menu` appends a new ID or replaces an existing menu in place. Factory
IDs are `application`, `File`, `Edit`, `Create`, `Pattern`, `View`, and `Help`. `menus/remove-menu` removes
one by ID. `menus/set-menus` replaces the complete list, including its order.
`menus/definition` returns the authored definitions, including predicates;
`menus/current-menus` returns the resolved configuration. Invalid definitions
return false with a diagnostic and leave the previous configuration intact.

To extend an existing menu:

```lisp
(let ((file (nth (filter (lambda (menu) (= (get menu :id) "File"))
                   (menus/definition)) 0)))
  (menus/register-menu
    (merge file :items
      (append (get file :items)
        (list (dict :id "file-extra" :label "My Action"
                :on-select (lambda () (status "My action ran"))))))))
```

## Definition fields

| Field | Meaning |
|---|---|
| `:id` | Nonempty unique ID across the entire tree. |
| `:label` | Display label. |
| `:items` | List of child entries; required for top-level menus. Nested submenus are supported. `nil` is a separator inside a menu. |
| `:on-select` | Zero-argument function, or a fully qualified global function name string. Required on ordinary leaf items. |
| `:shortcut` | Optional map with `:key` and `:modifiers`. |
| `:enabled` | Boolean, default true. Disabling a submenu disables its descendants. |
| `:checked` | Boolean checkmark for an ordinary action item. |
| `:checked-when` | Reactive predicate producing the checkmark. |
| `:items-when` | Reactive function producing submenu entries, used by Open Recent. |
| `:enabled-when` | High-level registry predicate, evaluated reactively to produce `:enabled`. |
| `:native-only` | Exclude a top-level menu from the toolbar fallback, as for the application menu. |
| `:role` | A system action instead of `:on-select`: `:services`, `:hide`, `:hide-others`, `:show-all`, or `:quit`. |

A leaf has exactly one of `:on-select` or `:role`. Submenus have neither and do
not accept shortcuts. Trees are limited to 16 submenu levels. Callbacks run on
the UI thread and should return promptly; use established host commands for
application operations.

Shortcut modifiers are `:primary` (Command on macOS, Control elsewhere),
`:command`, `:super`, `:control`, `:shift`, and `:alt`. Keys are logical characters
such as `"+"`, or named keys such as `"Enter"` and `"F1"`. These configure native
accelerators; independent editor/mode bindings still use `bind-key` and
`mode-bind-key`. The toolbar displays the platform-appropriate shortcut hints.
System roles other than Quit use OS-managed shortcuts and enabled states. Put
those roles in a native-only menu. Quit follows the application's normal shutdown
path rather than terminating the process directly.

## Reactive enabling

```lisp
(dict :id "my-pattern-action" :label "My Pattern Action"
  :enabled-when (lambda ()
    (let ((context (native-menu-context)))
      (and (not (get context :blocked))
           (get context :ui-view)
           (not (get context :text-input))
           (> SEQ.num-tracks 0))))
  :on-select (lambda () (host-command "menu-pattern-double" (dict))))
```

The registry uses `(observe ...)`, a nonvisual reactive effect. Observers run
once when defined and again when their dependencies change, discard their return
value, and never emit widget trees or pause because a UI panel is hidden.

Predicates should be pure. Their reactive reads, including SEQ fields and
`native-menu-context`, determine when they re-evaluate. Enabled-state and callback
updates reuse existing native objects. Label, shortcut, ordering, or structure
changes replace the native tree; queued events from removed items are discarded.

## Low-level natives

| Native | Purpose |
|---|---|
| `(native-menu-validate menus)` | Validate a resolved tree without changing it. |
| `(native-menu-set! menus)` | Publish a complete resolved tree; an empty list clears it. |
| `(native-menu-definition)` | Reactive snapshot of the resolved desired tree. |
| `(native-menu-supported?)` | Whether this UI runtime has a native backend attached. |
| `(native-menu-installed?)` | Whether a native tree has actually been installed. |
| `(native-menu-context)` | Reactive map containing `:blocked`, `:text-input`, and `:ui-view`. |
| `(native-menu-shortcut-label shortcut)` | Format a shortcut for this platform. |
| `(native-menu-activate id)` | Queue a callback or Quit action by ID, respecting ancestor and item enabling. |

The high-level registry uses these natives internally. Configure through
`eseq.menus` when using the factory registry; directly publishing a low-level
tree does not change that registry and a subsequent registry update will replace
it. A custom menu system can use the natives directly. The low-level schema takes
resolved boolean `:enabled` values; `:enabled-when` is evaluated by `eseq.menus`.

## Factory actions

File includes project shortcuts, the recent project list, and a macOS files/folders
picker feeding the existing sample import review. Recent projects are recorded
only after a successful open or save, are deduplicated, and retain twelve entries
in the user data directory. Opening a recent project with unsaved changes asks
before discarding them.

Edit dispatches to the focused widget/text editor or the sequencer selection.
No Undo/Redo entries are installed. View checkmarks follow panel visibility.
Pattern transposition preserves polyphonic intervals, note durations and delays;
left/right shift wraps the complete step payload. Clearing requires confirmation.
Select an individual track for these transforms; rack-level double/half retains
its existing grouped behavior.

Help > Search Commands searches enabled menu actions by their menu path and
shows configured shortcut hints. Type to filter, use Up/Down or Tab to choose,
Enter to run, and Escape to cancel. It preserves the original editor focus.
The lower-level Lisp/global command search remains available through M-x.
