;; ui/theme-editor.lisp -- the *theme-editor* buffer: every theme slot of the
;; theme being edited as a row with its color swatch. Clicking a swatch opens
;; a color picker; each change is applied to the live theme immediately, so
;; the rest of the UI previews it. Save writes the edited slots back into the
;; theme file in place (comments and layout kept); Save As copies the file
;; under a new name with every slot pinned, so the copy stands on its own.
;;
;; The buffer opens in the right-side pane inspect mode uses (M-x Edit
;; Theme). A theme file only sets some slots; the rest keep whatever the
;; previously loaded theme left behind, so rows the file does not set are
;; marked "inherited". Editing one adds it to the file on Save.
;;
;; Render root: registers the buffer and its mode at top level. Import it
;; only from main.lisp.
(module eseq.theme-editor)

(export edit-theme
        theme-editor-close
        theme-editor-save
        theme-editor-save-as
        theme-editor-revert
        open-picker
        close-picker
        set-picker-hsva
        pick-color
        start-pick
        apply-pick
        clear-pick)

;; ── State ──

;; Registry name and absolute path of the theme being edited.
(defstate te-theme "")
(defstate te-file "")
;; Row snapshot the table renders: dicts of :name :color :widget :folder
;; :in-file :edited.
;; Rebuilt on open, on picker close and after a save, never per drag, so a
;; drag re-renders the picker alone.
(defstate te-rows (list))
;; Slot colors at the last load/save, as (name color) pairs: what Revert
;; restores and what "edited" compares against.
(defstate te-baseline (list))
;; Unsaved edits as (name color) pairs, in edit order.
(defstate te-edits (list))
(defstate te-filter "")
(defstate te-show-inherited true)
;; Picker: the slot being edited ("" = closed), its HSVA, the color it had
;; when the picker opened, and the slots sharing that color.
(defstate te-picking "")
(defstate te-h 0)
(defstate te-s 0)
(defstate te-v 0)
(defstate te-a 1)
(defstate te-pick-original (list 0 0 0 1))
(defstate te-twins (list))
(defstate te-link-twins false)
(defstate te-hex-draft "")
;; The theme's distinct colors as (color count) pairs, taken when the picker
;; opens (te-rows does not change while it is open).
(defstate te-palette (list))
;; Save As name entry (shown inline in the header while non-nil).
(defstate te-save-as-open false)
(defstate te-save-as-name "")
;; Theme pick result: one dict per widget under the clicked point, from the
;; clicked widget outward, :label and :slots. Empty = no pick filter.
(defstate te-pick-groups (list))

(def buffer-name "*theme-editor*")

;; ── Small helpers ──

(def member? (xs x)
  (reduce (lambda (acc y) (if (= y x) true acc)) false xs))

(def pair-get (pairs name)
  (reduce (lambda (acc pair) (if (= (nth pair 0) name) (nth pair 1) acc)) nil pairs))

(def registry-entry (name)
  (find-by-key (seq-theme-registry) :name name))

(def theme-path (entry)
  (let ((file (get entry :file)))
    (if (string-starts-with? file "@/")
      (seq-factory-path (substring file 2))
      file)))

(def slot-color (name)
  (let ((edited (pair-get te-edits name)))
    (if (= edited nil) (pair-get te-baseline name) edited)))

;; ── Loading ──

(def snapshot-rows ()
  (let ((edited (map |pair| (nth pair 0) te-edits)))
    (set! te-rows
      (map |row| (merge row :edited (member? edited (get row :name)))
           (theme-slots te-file)))))

(def load-theme-for-editing (name)
  (let ((entry (registry-entry name)))
    (if (= entry nil)
      (status (str "Unknown theme: " name))
      (do
        (seq-load-theme-file name (get entry :file))
        (set! te-theme name)
        (set! te-file (theme-path entry))
        (set! te-edits (list))
        (set! te-picking "")
        (set! te-pick-groups (list))
        (snapshot-rows)
        (set! te-baseline (map |row| (list (get row :name) (get row :color)) te-rows))))))

(def edit-theme ()
  :doc "Edit the current theme's colors in a side pane, with live preview."
  :command "Edit Theme"
  :icon :swatch
  :group "View"
  (do
    (load-theme-for-editing seq-current-theme)
    (show-in-side-pane buffer-name)))

(def theme-editor-close ()
  (do
    (set! te-picking "")
    (close-window-for buffer-name)))

(def switch-theme (name)
  (do
    (if (> (len te-edits) 0)
      (status (fmt "Discarded {} unsaved slot edits" (len te-edits)))
      nil)
    (load-theme-for-editing name)))

;; ── Editing ──

;; Apply one color to `name` (and to its twins when linked), live.
(def write-slots (names color)
  (do
    (apply-theme-slots (map |name| (list name color) names))
    (set! te-edits
      (append (filter |pair| (not (member? names (nth pair 0))) te-edits)
              (map |name| (list name color) names)))))

(def picker-targets ()
  (if te-link-twins (cons te-picking te-twins) (list te-picking)))

(def picker-color ()
  (let ((rgb (hsv->rgb te-h te-s te-v)))
    (list (nth rgb 0) (nth rgb 1) (nth rgb 2) te-a)))

(def set-picker-hsva (h s v a)
  (do
    (set! te-h h)
    (set! te-s s)
    (set! te-v v)
    (set! te-a a)
    (let ((color (picker-color)))
      (do
        (set! te-hex-draft (color->hex color))
        (write-slots (picker-targets) color)))))

(def load-picker-color (color)
  (let ((hsv (rgb->hsv (nth color 0) (nth color 1) (nth color 2))))
    (do
      ;; Keep the hue when the new color is grey, so dragging back out of
      ;; the grey edge returns to the hue the user was on.
      (if (> (nth hsv 1) 0) (set! te-h (nth hsv 0)) nil)
      (set! te-s (nth hsv 1))
      (set! te-v (nth hsv 2))
      (set! te-a (if (> (len color) 3) (nth color 3) 1))
      (set! te-hex-draft (color->hex color)))))

;; Set the picker to a color (palette swatch, hex entry, revert).
(def pick-color (color)
  (do
    (load-picker-color color)
    (write-slots (picker-targets) (picker-color))))

(def open-picker (name)
  (let ((color (slot-color name)))
    (do
      (set! te-picking name)
      (set! te-pick-original color)
      (set! te-link-twins false)
      (set! te-twins
        (let ((hex (color->hex (pair-get te-baseline name))))
          (map |pair| (nth pair 0)
               (filter |pair| (and (not (= (nth pair 0) name))
                                   (= (color->hex (nth pair 1)) hex))
                       te-baseline))))
      (set! te-palette (distinct-colors (map |row| (get row :color) te-rows)))
      (set! te-h 0)
      (load-picker-color color))))

(def close-picker ()
  (do
    (set! te-picking "")
    (snapshot-rows)))

(def set-link-twins (linked)
  (do
    (set! te-link-twins linked)
    ;; Linking applies the current color to the twins now; unlinking puts
    ;; them back to what they were.
    (if linked
      (write-slots te-twins (picker-color))
      (do
        (apply-theme-slots (map |name| (list name (pair-get te-baseline name)) te-twins))
        (set! te-edits (filter |pair| (not (member? te-twins (nth pair 0))) te-edits))))))

(def revert-pick ()
  (pick-color te-pick-original))

;; Typing or pasting in the hex field applies as soon as the text is a whole
;; hex color, and leaves the text as typed (no "#" added mid-edit).
(def hex-typed (text)
  (let ((color (hex->color text)))
    (do
      (if (= color nil) nil (pick-color color))
      (set! te-hex-draft text))))

(def submit-hex (text)
  (if (= (hex->color text) nil)
    (status (str "Not a hex color: " text))
    (pick-color (hex->color text))))

;; ── Theme pick ──

;; Hover the UI to see which slots each widget paints with; a click narrows
;; the table to those slots, grouped by widget.
(def start-pick ()
  (theme-pick-start (lambda (groups) (apply-pick groups))))

(def apply-pick (groups)
  (do
    (set! te-pick-groups groups)
    (set! te-filter "")
    (if (= (len groups) 0)
      (status "That element reads no theme slots")
      (status (fmt "Showing the theme slots of {} element(s)" (len groups))))))

(def clear-pick ()
  (set! te-pick-groups (list)))

;; ── Save / revert ──

(def theme-editor-revert ()
  (do
    (apply-theme-slots te-baseline)
    (set! te-edits (list))
    (set! te-picking "")
    (snapshot-rows)
    (status "Theme edits reverted")))

(def theme-editor-save ()
  (if (= (len te-edits) 0)
    (status "No theme changes to save")
    (do
      (theme-file-save te-file te-edits)
      (set! te-edits (list))
      (snapshot-rows)
      (set! te-baseline (map |row| (list (get row :name) (get row :color)) te-rows))
      (status (str "Saved " te-file)))))

;; "My Theme 2" -> "my-theme-2"
(def slugify (text)
  (reduce (lambda (acc part) (if (= acc "") part (str acc "-" part)))
    ""
    (filter |part| (not (= part ""))
            (string-split (string-downcase (string-trim text)) " "))))

;; The live theme already carries every edit.
(def colored-slot-pairs ()
  (map |row| (list (get row :name) (get row :color))
       (filter |row| (= (get row :widget) nil) (theme-slots))))

(def theme-editor-save-as (title)
  (let ((slug (slugify title)))
    (if (= slug "")
      (status "Theme name is empty")
      (let ((path (seq-factory-path (str "ui/themes/" slug ".lisp"))))
        (if (file-exists? path)
          (status (str "A theme named " slug " already exists"))
          (do
            ;; Pin every color slot so the copy does not inherit from
            ;; whichever theme happens to be loaded before it.
            (theme-file-save-as te-file path (str slug "-theme") (colored-slot-pairs))
            (set! te-save-as-open false)
            (set! te-save-as-name "")
            (load-theme-for-editing slug)
            (status (str "Saved new theme " slug " (" path ")"))))))))

;; ── Widgets ──

(def visible-rows ()
  (let ((needle (string-downcase te-filter)))
    (filter |row| (and (or te-show-inherited (get row :in-file))
                       (or (= needle "") (string-contains? (get row :name) needle)))
            te-rows)))

;; ── Slot tree ──
;; The slots as a `tree`, like the browser: each leaf carries its color as a
;; `:swatch` chip and its hex / inherited / edited state as `:detail`. A click
;; (on-select) opens the picker; widget-valued slots are listed but inert.

(def slot-detail (row)
  (let ((name (get row :name))
        (edited (get row :edited))
        (base (if (= (get row :widget) nil)
                (color->hex (get row :color))
                (str "widget " (get row :widget)))))
    (str base
      (if (get row :in-file) "" "  inherited")
      (if edited "  edited" ""))))

(def slot-item (row)
  (if (= (get row :widget) nil)
    (dict :label (get row :name) :name (get row :name) :slot (get row :name)
          :swatch (get row :color) :detail (slot-detail row))
    (dict :label (get row :name) :name (get row :name) :detail (slot-detail row))))

;; Folders come from `theme-slots` :folder: a slot's first name segment when
;; at least three slots share it (patcher-*, mod-*, eq8-*), else "general".
(def folder-items (rows)
  (let ((folders (reduce (lambda (acc row)
                           (let ((folder (get row :folder)))
                             (if (member? acc folder) acc (append acc (list folder)))))
                         (list) rows)))
    (map |folder| (dict :label folder :folder folder
                        :children (map |row| (slot-item row)
                                       (filter |row| (= (get row :folder) folder) rows)))
         folders)))

(def row-named (name)
  (find-by-key te-rows :name name))

(def pick-items ()
  (map |i| (let ((group (nth te-pick-groups i)))
             (dict :label (if (= i 0) (str (get group :label) " (clicked)") (get group :label))
                   :folder (str "pick-" i)
                   :children (map |row| (slot-item row)
                                  (filter |row| (not (= row nil))
                                          (map |name| (row-named name) (get group :slots))))))
       (range 0 (len te-pick-groups))))

(def select-slot (item)
  (let ((slot (get item :slot)))
    (if (= slot nil) nil (open-picker slot))))

(def table ()
  (subtree :key "te-table"
    (let ((picking (> (len te-pick-groups) 0))
          (rows (visible-rows)))
      (v-stack :width :fill :gap 0.2
        (if picking nil
          (label (fmt "{} slots" (len rows)) :font-size 8.5 :color :dim :bg :transparent))
        (tree :key (if picking "te-pick-tree" "te-tree")
          :width :fill
          :background-color :buffer-bg
          :items (if picking (pick-items) (folder-items rows))
          :expand-all (or picking (not (= te-filter "")))
          :font-size 11
          :focusable true
          :on-select (lambda (item) (select-slot item))
          :on-activate (lambda (item) (select-slot item)))))))

(def theme-choices ()
  (map |entry| (get entry :name) (seq-theme-registry)))

(def header ()
  (v-stack :width :fill :gap 0.4
    (h-stack :width :fill :gap 0.5 :v-align :center
      (label "Theme" :font-size 13 :color :white :bg :transparent)
      (dropdown :key "te-theme-choice" :width 14 :height 1.3 :font-size 10
        :value te-theme :options (theme-choices) :filterable true
        :on-change (lambda (picked) (switch-theme picked)))
      (box :flex 1 :height 1 :bg :transparent)
      (button "×" :width 2 :corner-radius 32 :key "te-close" :on-click |x y r| (theme-editor-close)))
    (h-stack :width :fill :gap 0.4 :v-align :center
      (button "Save" :key "te-save" :variant :primary
        :font-size 10
        :disabled (= (len te-edits) 0)
        :on-click |x y r| (theme-editor-save))
      (button "Save As…" :key "te-save-as"
        :font-size 10
        :on-click |x y r| (set! te-save-as-open (not te-save-as-open)))
      (button "Revert" :key "te-revert"
        :font-size 10
        :disabled (= (len te-edits) 0)
        :on-click |x y r| (theme-editor-revert))
      (label (if (= (len te-edits) 0) "saved" (fmt "{} unsaved" (len te-edits)))
        :font-size 9 :color (if (= (len te-edits) 0) :dim :accent) :bg :transparent))
    (if te-save-as-open
      (h-stack :width :fill :gap 0.4 :v-align :center
        (text-input :key "te-save-as-name" :flex 1 :height 1.4 :font-size 10
          :value te-save-as-name :placeholder "New theme name" :auto-focus true
          :on-change |v| (set! te-save-as-name v)
          :on-submit |v| (theme-editor-save-as v))
        (button "Create" :key "te-save-as-go" :variant :primary
          :font-size 10
          :on-click |x y r| (theme-editor-save-as te-save-as-name)))
      nil)
    (if (> (len te-pick-groups) 0)
      (h-stack :width :fill :gap 0.4 :v-align :center
        (label "Picked element" :font-size 10 :color :accent :bg :transparent)
        (box :flex 1 :height 1 :bg :transparent)
        (button 
          "Pick again" 
          :font-size 10
          :key "te-pick-again" :on-click |x y r| (start-pick))
        (button "Show all" :key "te-pick-clear" :on-click |x y r| (clear-pick)))
      (h-stack :width :fill :gap 0.4 :v-align :center
        (button 
          "Pick" 
          :font-size 10
          :key "te-pick" :on-click |x y r| (start-pick))
        (text-input :key "te-filter" :flex 1 :height 1.4 :font-size 10
          :value te-filter :placeholder "Filter slots…"
          :on-change |v| (set! te-filter v))
        (toggle :key "te-show-inherited" :value te-show-inherited
          :on-change |v| (set! te-show-inherited v))
        (label "inherited" :font-size 9 :color :dim :bg :transparent)))
    ))

;; ── Picker modal ──

(def palette-swatch (entry current-hex)
  (let ((color (nth entry 0))
        (hex (color->hex (nth entry 0))))
    (box :key (str "te-palette-" hex)
      :width 1.8 :height 0.9 :background-color color :corner-radius 2
      :border-color (if (= hex current-hex) :white :border-dim)
      :border-width (if (= hex current-hex) 2 1)
      :on-click (lambda (info) (pick-color color)))))

(def picker-panel ()
  (let ((color (picker-color))
        (current-hex (color->hex (picker-color)))
        (palette te-palette))
    (v-stack :width :fill :height :fill :gap 0.5 :padding 0.4
      (h-stack :width :fill :gap 0.5 :v-align :center
        (label te-picking :font-size 12 :color :white :bg :transparent :flex 1)
        (button "Done" :key "te-picker-done" :variant :primary
          :on-click |x y r| (close-picker)))
      (color-picker :key "te-color-picker" :width :fill :height 9
        :h te-h :s te-s :v te-v :a te-a
        :on-change (lambda (h s v a) (set-picker-hsva h s v a)))
      (h-stack :width :fill :gap 0.4 :v-align :center
        (label "was" :font-size 8 :color :dim :bg :transparent)
        (box :key "te-before" :width 3 :height 1.4 :background-color te-pick-original :corner-radius 3
          :border-color :border-dim :border-width 1
          :on-click (lambda (info) (revert-pick)))
        (box :width 3 :height 1.4 :background-color color :corner-radius 3 :border-color :border-dim :border-width 1)
        (text-input :key "te-hex" :flex 1 :height 1.4 :font-size 10
          :value te-hex-draft :select-all-on-focus true
          :on-change |v| (hex-typed v)
          :on-submit |v| (submit-hex v))
        (label (fmt "α {}" (/ (round (* te-a 100)) 100)) :font-size 9 :color :dim :bg :transparent))
      (if (> (len te-twins) 0)
        (h-stack :width :fill :gap 0.4 :v-align :center
          (toggle :key "te-link-twins" :value te-link-twins
            :on-change |v| (set-link-twins v))
          (label (fmt "Also change the {} other slots with this color" (len te-twins))
            :font-size 9 :color :fg :bg :transparent))
        nil)
      (label (fmt "Colors in this theme ({})" (len palette)) :font-size 9 :color :dim :bg :transparent)
      (scroll :key "te-palette-scroll" :width :fill :flex 1
        (wrap :width :fill :gap 0.25 :row-gap 0.2
          (each palette |entry| (palette-swatch entry current-hex)))))))

(def picker-modal ()
  (modal :is-open (not (= te-picking "")) :on-close (lambda () (close-picker))
      :width-px 440 :height-px 820 :scrim-color :transparent
    (box :debug-name "theme-editor-picker" :width :fill :height :fill :bg :transparent
      (if (= te-picking "")
        (box :width 0 :height 0 :bg :transparent)
        (subtree :key "te-picker" (picker-panel))))))

(def root-widget ()
  (box :width :fill :height :fill :background-color :buffer-bg
    (v-stack :width :fill :height :fill :gap 0.5 :padding 0.6
      (header)
      (scroll :key "te-scroll" :width :fill :flex 1
        (box :width :fill :padding-right 2.0
          (table)))
      (picker-modal))))

(effect-buffer "*theme-editor*"
  (root-widget))

(define-mode "eseq.theme-editor/theme-editor-mode" :read-only true)
(mode-bind-key "eseq.theme-editor/theme-editor-mode" "q" "eseq.theme-editor/theme-editor-close")
(mode-bind-key "eseq.theme-editor/theme-editor-mode" "C-x C-s" "eseq.theme-editor/theme-editor-save")
(set-buffer-mode-for "*theme-editor*" "eseq.theme-editor/theme-editor-mode")
