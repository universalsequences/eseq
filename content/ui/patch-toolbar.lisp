;; ui/patch-toolbar.lisp — The patch editor's toolbar (*patch-toolbar*).
;;
;; One fixed row over the patch canvas (docs/patcher-workspace-spec.md §3):
;; the instrument's name, its release, Instrument | Free, compile status, the
;; Patch | Code surface switch, and Save / Save as… / Done. It replaces the
;; Finalize / Save / Fork stack the editor used to put in *samples*, which a
;; panel toggle could hide. Saving and leaving are separate: Save writes and
;; stays, Done goes back to the sequencer (asking first when there are unsaved
;; changes).
;;
;; The editor state is the host's `editor` kind; the toolbar's own state is the
;; `patch-toolbar` singleton below (the name being typed for a draft, and
;; whether Done is asking about unsaved changes).
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers the
;; *patch-toolbar* effect-buffer at top level. NEVER import it from a library
;; file.
(module eseq.patch-toolbar)
(import eseq.kinds :refer (editor))
(import eseq.view-kit :refer (open-menu! menu-of))

(export patch-toolbar
        status-row
        save!
        save-as!
        commit-name!
        done!)

(def-kind patch-toolbar
  :key ()
  :state ((name "")
          ;; The buffer the name was typed for: a new session starts with an
          ;; empty name instead of the last one's.
          (buffer "")
          (confirming-done false)
          ;; The Save-as-name modal a draft's Save opens, and whether its
          ;; Save then leaves (Done's "Save").
          (naming false)
          (naming-done false)
          ;; Save as… forks into a draft session; its name is asked once
          ;; that session is up.
          (name-after-fork false)))

;; The release badge's menu.
(def-kind release-menu
  :key ()
  :state ((open false)
          (at :point :default nil)))

(def draft? ()
  (or (= editor.mode "new-instrument") (= editor.mode "new-effect")))

(def editing-existing? ()
  (or (= editor.mode "edit-instrument") (= editor.mode "edit-effect")))

(def instrument? ()
  (or (= editor.mode "new-instrument") (= editor.mode "edit-instrument")))

(def macro-action? ()
  (or (= editor.active-macro-action "save-to-library")
      (= editor.active-macro-action "fork")))

(def busy? ()
  (or editor.canceling
      (= editor.error "Preview compiling...")))

;; Reset the typed name when a different session opens.
(def sync-session! ()
  (if (= patch-toolbar.buffer editor.buffer)
    nil
    (do
      (set! patch-toolbar.buffer editor.buffer)
      (set! patch-toolbar.name "")
      (set! patch-toolbar.confirming-done false)
      (set! patch-toolbar.naming-done false)
      (set! patch-toolbar.naming (and patch-toolbar.name-after-fork (draft?)))
      (if (draft?) (set! patch-toolbar.name-after-fork false) nil))))

;; ── Actions ──

(def save! ()
  :doc "Save the patch being edited, staying in the editor. A draft asks for its name first; a saved patch saves at once and a toast says what it did."
  (do
    (set! patch-toolbar.confirming-done false)
    (if (macro-action?)
      (host-command "save-active-editor-macro" (dict))
      (if (draft?)
        (open-naming! false)
        (host-command "editor-save" (dict))))))

(def open-naming! (done)
  (do
    (set! patch-toolbar.naming-done done)
    (set! patch-toolbar.naming true)))

(def commit-name! ()
  :doc "Save the draft under the name typed in the naming modal."
  (if (= patch-toolbar.name "")
    nil
    (do
      (set! patch-toolbar.naming false)
      (host-command "editor-save"
        (dict :name patch-toolbar.name :done patch-toolbar.naming-done)))))

(def save-as! ()
  :doc "Save the patch being edited as a new instrument or effect, asking its name."
  (do
    (set! patch-toolbar.confirming-done false)
    (set! patch-toolbar.name "")
    ;; A draft has no saved copy to fork (the host's editor-save-as is a
    ;; no-op on one), so its Save as… is its first Save: ask the name now.
    (if (draft?)
      (open-naming! false)
      (do
        (set! patch-toolbar.name-after-fork true)
        (host-command "editor-save-as" (dict))))))

(def done! ()
  :doc "Leave the patch editor, asking first when there are unsaved changes."
  (if (and editor.dirty (not patch-toolbar.confirming-done))
    (set! patch-toolbar.confirming-done true)
    (do
      (set! patch-toolbar.confirming-done false)
      (host-command "editor-done" (dict)))))

(def discard! ()
  (do
    (set! patch-toolbar.confirming-done false)
    (host-command "cancel-editor" (dict))))

(def save-and-done! ()
  (do
    (set! patch-toolbar.confirming-done false)
    (if (draft?)
      (open-naming! true)
      (host-command "editor-save" (dict :done true)))))

;; ── Pieces ──

(defwidget compile-spinner
  ;; Sized to sit inside `status-row`'s 1.35-row height — the
  ;; row does not clip, so an oversized spinner paints over its neighbours.
  ;; The dot row spans x = -0.72..0.72 in local space, so keep the box wider
  ;; than it is tall or the outer dots run off the edges.
  :width 3.0 :height 1.25
  :animates true
  :shader
  (let ((phase (* itime 5.4))
        (p0 (+ 0.45 (* 0.55 (sin phase))))
        (p1 (+ 0.45 (* 0.55 (sin (- phase 0.75)))))
        (p2 (+ 0.45 (* 0.55 (sin (- phase 1.5)))))
        (p3 (+ 0.45 (* 0.55 (sin (- phase 2.25)))))
        (p4 (+ 0.45 (* 0.55 (sin (- phase 3.0)))))
        (r0 (+ 0.17 (* 0.12 p0)))
        (r1 (+ 0.17 (* 0.12 p1)))
        (r2 (+ 0.17 (* 0.12 p2)))
        (r3 (+ 0.17 (* 0.12 p3)))
        (r4 (+ 0.17 (* 0.12 p4))))
    (sdf/layer
      (sdf/fill (sdf/translate -0.72 0 (sdf/circle r0))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p0)))))
      (sdf/fill (sdf/translate -0.36 0 (sdf/circle r1))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p1)))))
      (sdf/fill (sdf/translate 0 0 (sdf/circle r2))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p2)))))
      (sdf/fill (sdf/translate 0.36 0 (sdf/circle r3))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p3)))))
      (sdf/fill (sdf/translate 0.72 0 (sdf/circle r4))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p4))))))))

(def status-row (text color)
  ;; :center, not :baseline — the spinner is a bare SDF with no text baseline
  ;; to align against.
  (h-stack :height 1.35 :gap 0.5 :align :center
    (compile-spinner :width 2.8 :height 1.15)
    (label text
      :font-size 9
      :color color
      :bg :transparent)))


(def separator ()
  (box :width 0.08 :height 1.4 :background-color :mixer-strip-border))

;; Centered on the row's buttons (a bare label sits on the text baseline).
(def toolbar-label (text &key (color :fg) (size 11))
  (label text :font-size size :color color :bg :transparent
    :height 1.4 :v-align :center))

(def segment (text on on-click)
  (button text
    :background-color (if on :mixer-strip-selected-bg :transparent)
    :border-color :mixer-strip-border
    :border-width 1
    :height 1.4
    :font-size 10.5
    :on-click on-click
    :color (if on :fg :dim)))

;; Inside a macro view: back to the patch root (the breadcrumb's first
;; segment).
(def back-button ()
  (let ((open editor.open-macro))
    (if (= open "")
      (box :width 0 :height 0)
      (button "‹"
        :background-color :transparent
        :border-width 0
        :height 1.4
        :font-size 14
        :on-click |x y r| (patcher-breadcrumb-pop! 0)
        :color :fg))))

(def name-field ()
  (if (draft?)
    (toolbar-label (if (instrument?) "untitled instrument" "untitled effect") :color :dim :size 12.5)
    (toolbar-label (if (= editor.title "") editor.buffer editor.title) :size 12.5)))

;; The naming modal: a draft's Save asks for its name here, so the toolbar
;; need not carry an always-on name field. Saving a saved patch has nothing
;; to ask, so it saves at once and toasts what it did (no confirm modal).
(def naming-title ()
  (if (instrument?) "Save instrument" "Save effect"))

;; Laid out here rather than with the modal's :title header, which sits tight
;; against the top edge: the title gets real room above it, the buttons a
;; bottom margin like the sides (a native macOS sheet's balance). Each
;; variant's height fits its content, so the margins come out even.
(def naming-modal ()
  (modal :key "patch-toolbar-naming" :is-open patch-toolbar.naming
    :on-close (lambda () (set! patch-toolbar.naming false))
    :width-px 460 :height-px 260
    (if patch-toolbar.naming
      ;; Cells are about twice as tall as they are wide: 0.75 extra columns
      ;; each side and 0.35 rows above the title land near equal pixels. A
      ;; stack ignores :padding-left, so the sides are spacer columns.
      (h-stack :width :fill :height :fill :gap 0
        (box :width 0.75 :height 1)
      (v-stack :flex 1 :width 1 :height :fill :gap 0
        (box :height 0.35 :width 1)
        (label (naming-title) :bg :transparent :font-size 14 :color :fg)
        (box :height 0.7 :width 1)
        (text-input :key "patch-toolbar-name" :width :fill :height 1.5 :font-size 12
          :value patch-toolbar.name
          :placeholder (if (instrument?) "instrument name" "effect name")
          :on-change (lambda (v) (set! patch-toolbar.name v))
          :on-submit (lambda () (commit-name!)))
        (box :height 0.9 :width 1)
        (h-stack :width :fill :gap 0.8 :align :center
          (box :height 0 :flex 1)
          (button "Cancel" :key "cancel" :variant :ghost
            :on-click |event| (set! patch-toolbar.naming false))
          (button (if patch-toolbar.naming-done "Save & Close" "Save") :key "commit" :variant :primary
            :disabled (= patch-toolbar.name "")
            :on-click |event| (commit-name!)))
        (box :height 0.45 :width 1))
        (box :width 0.75 :height 1))
      (box :height 0 :width 0))))

;; v3 while a saved release is open; nothing for drafts and unversioned
;; instruments. It opens the release menu: every release with the tracks of
;; the open project playing it, and Restore for an older one (a new release
;; holding its files; offered while the patch is saved).
(def release-label (i)
  (let ((n (nth editor.releases i))
        (tracks (nth editor.release-tracks i)))
    (str "v" n
      (if (= n editor.release) "  (open)" "")
      (if (> tracks 0) (str "  ·  " tracks (if (= tracks 1) " track" " tracks")) ""))))

(def release-menu-items ()
  (map (lambda (i)
         (let ((n (nth editor.releases i)))
           (if (= n editor.release)
             (menu-item (release-label i) :key (str "release-" n) :disabled true)
             (menu-item (str "Restore " (release-label i))
               :key (str "release-" n)
               :disabled editor.dirty
               :on-select (lambda (e)
                            (do (set! release-menu.open false)
                                (host-command "editor-restore-release" (dict :release n))))))))
       (range 0 (len editor.releases))))

(def release-badge ()
  (if (> editor.release 0)
    (box :padding 0.2 :corner-radius 6 :background-color :mixer-control-bg
      :on-click (lambda (event) (open-menu! release-menu event))
      (toolbar-label (str "v" editor.release " ▾") :size 10 :color :dim))
    (box :width 0 :height 0)))

(def run-mode-toggle ()
  (if (instrument?)
    (h-stack :gap 0 :align :center
      (segment "Instrument" (= editor.run-mode "instrument")
        |x y r| (set! editor.run-mode "instrument"))
      (segment "Free" (= editor.run-mode "free_patch")
        |x y r| (set! editor.run-mode "free_patch")))
    (box :width 0 :height 0)))

(def status ()
  (if editor.canceling
    (status-row "Canceling…" :dim)
    (if (= editor.error "Preview compiling...")
      (status-row "Compiling…" :dim)
      (if (not (= editor.error ""))
        (label editor.error :font-size 10 :color :red :bg :transparent :max-width 40)
        (if editor.dirty
          (toolbar-label "• edited" :color :dim :size 10)
          (box :width 0 :height 0))))))

;; Patch | Code, for an existing instrument or effect (a draft has no source
;; file to eject to yet). Eval stays a code-surface key (C-c C-c) and button.
(def surface-toggle ()
  (if (editing-existing?)
    (h-stack :gap 0 :align :center
      (segment "Patch" (= editor.surface "patch")
        |x y r| (if (= editor.surface "patch") nil (host-command "promote-editor-to-patch" (dict))))
      (segment "Code" (= editor.surface "code")
        |x y r| (if (= editor.surface "code") nil (host-command "eject-editor-to-code" (dict)))))
    (box :width 0 :height 0)))

(def eval-button ()
  (if (= editor.surface "code")
    (button "Eval"
      :variant :secondary
      :height 1.4
      :font-size 10.5
      :on-click |x y r| (host-command "evaluate-editor-source" (dict))
      :color :fg)
    (box :width 0 :height 0)))

(def action-button (text variant on-click)
  (button text
    :variant variant
    :height 1.5
    :font-size 11.5
    :on-click on-click
    :color (if (= variant :primary) :browser-primary-fg :fg)))

(def actions ()
  (if (busy?)
    (box :width 0 :height 0)
;; The question lapses once nothing is unsaved (a Save landed).
    (if (and patch-toolbar.confirming-done editor.dirty)
      (h-stack :gap 0.5 :align :center
        (toolbar-label "Unsaved changes" :color :dim :size 10.5)
        (action-button "Save" :primary |x y r| (save-and-done!))
        (action-button "Discard" :secondary |x y r| (discard!))
        (action-button "Keep editing" :secondary |x y r| (set! patch-toolbar.confirming-done false)))
      (h-stack :gap 0.5 :align :center
        (action-button (if (macro-action?)
                         (if (= editor.active-macro-action "fork") "Fork Macro" "Save Macro to Library")
                         "Save")
          :primary |x y r| (save!))
        (if (and (editing-existing?) (not (macro-action?)))
          (action-button "Save as…" :secondary |x y r| (save-as!))
          (box :width 0 :height 0))
        (action-button "Done" :secondary |x y r| (done!))))))

;; `root / dsp.lisp / <macro>`, each segment clickable to go back to it. The
;; canvas no longer draws its own (patcher-set-breadcrumb-overlay! below).
;; patcher-breadcrumb is not reactive: reading editor.open-macro re-renders
;; this when a macro view opens or closes.
(def breadcrumb ()
  (let ((open editor.open-macro)
        (segs (patcher-breadcrumb)))
    (h-stack :gap 0.1 :align :center
      (each (range 0 (len segs)) |i|
        (h-stack :gap 0.1 :align :center
          (if (> i 0) (toolbar-label "/" :color :dim :size 10.5) (box :width 0 :height 0))
          (button (nth segs i)
            :background-color :transparent
            :border-width 0
            :height 1.4
            :font-size 10.5
            :color (if (= i (- (len segs) 1)) :fg :dim)
            :on-click |x y r| (patcher-breadcrumb-pop! i)))))))

(patcher-set-breadcrumb-overlay! false)

(set-buffer-mode-for "*patch-toolbar*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*patch-toolbar*"
  (do
    (sync-session!)
    ;; An h-stack root, not a box (a box lays out one child): the menu and
    ;; the modal ride beside the bar with zero footprint.
    (h-stack :width :fill :height :fill :gap 0
      (box :width :fill :height :fill :padding 0.3
        (h-stack :width :fill :height :fill :gap 0.7 :align :center
          (back-button)
          (name-field)
          (release-badge)
          (separator)
          (run-mode-toggle)
          (separator)
          (breadcrumb)
          (status)
          (box :flex 1 :height 1)
          (surface-toggle)
          (eval-button)
          (separator)
          (actions)))
      (subtree :key "patch-toolbar-release-menu"
        (apply menu-of release-menu (release-menu-items)))
      (subtree :key "patch-toolbar-naming-modal"
        (naming-modal)))))
