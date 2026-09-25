;; ui/effects/rack-groove.lisp — the drum rack panel's Groove section and its
;; Extract Groove modal (docs/rack-groove-spec.md, "UI").
;;
;; Sits beside the pad grid in the *fx* buffer's rack selection panel
;; (effects/buffers.lisp). A groove is the rack's extracted feel: the picker
;; lists This rack's grooves, then the Generic built-ins, then Off; the
;; Timing / Velocity / Random knobs scale it; the heatmap shows the active
;; groove as rows = pads, columns = slots, color = offset (amber early, blue
;; late), with filled (guessed) cells dimmed. The data and every host
;; command live in eseq.drum-rack-v2; this module is only the view.
(module eseq.effects.rack-groove)

(import eseq.drum-rack-v2 :as rack)
(import eseq.effects.state :as st)
(import eseq.effects.panel-frame :as pf)
(import eseq.grooves-tab :as gt)

(export panel
        extract-modal
        open-extract
        extract-open?
        commit-extract)

;; ── Extract Groove modal state ──────────────────────────────────────────
(defstate extract-open? false)
(defstate extract-gid -1)
(defstate extract-name "")
(defstate extract-bars "1 bar")
(defstate extract-resolution "1/16")
(defstate extract-quantize true)

;; Inline rename of the active rack groove (-1 = not renaming).
(defstate groove-renaming -1)
(defstate groove-rename-draft "")

(def open-extract (gid)
  (do
    (set! extract-gid gid)
    (set! extract-name
      (str "Groove " (+ 1 (len (or (get (or (rack/groove-state gid) (dict)) :grooves) (list))))))
    (set! extract-bars "1 bar")
    (set! extract-resolution "1/16")
    (set! extract-quantize true)
    (set! extract-open? true)))

(def close-extract () (set! extract-open? false))

(def commit-extract ()
  (do
    (rack/extract-groove extract-gid
      (if (= (len (string-trim extract-name)) 0) "Groove" extract-name)
      (if (= extract-bars "2 bars") 2 1)
      extract-resolution
      extract-quantize)
    (close-extract)))

(def begin-rename (state)
  (do
    (set! groove-renaming (get state :active-groove-id))
    (set! groove-rename-draft (get state :active-label))))

(def finish-rename (gid commit)
  (do
    (if (and commit (>= groove-renaming 0))
      (rack/rename-groove gid groove-renaming groove-rename-draft)
      nil)
    (set! groove-renaming -1)
    (set! groove-rename-draft "")))

;; ── Heatmap ─────────────────────────────────────────────────────────────
;; Cell colors (amber early, blue late, filled cells dimmed) are the Grooves
;; tab's `offset-color`, so both maps read the same.
(def heat-width 23)
(def heat-label-width 3.4)
(def heat-height 7.2)

;; The member track's name labels a pad row; a padless note falls back to
;; the note name the host published.
(def heat-row-label (gidx row)
  (let ((note (get row :pad-note)))
    (if (= note nil)
      "All"
      (let ((pad (rack/pad-at-note gidx note)))
        (let ((track (if (= pad nil) -1 (get pad :track))))
          (if (and (>= track 0) (< track (len (or SEQ.track-names (list)))))
            (substring (nth SEQ.track-names track) 0 7)
            (get row :label)))))))

(def heat-row (gidx gid row index cell-w row-h)
  (let ((cells (get row :cells))
        (measured (get row :measured)))
    (h-stack :key (str "rack-groove-heat-row-" gid "-" index) :gap 0 :align :center
      (box :width heat-label-width :height row-h :padding 0
        :v-align :center :h-align :start :bg :transparent
        (label (heat-row-label gidx row)
          :font-size 7 :v-align :center
          :color (if (get row :own) :white :dim) :bg :transparent))
      (each (range 0 (len cells)) |i|
        (box :key (str "rack-groove-heat-" gid "-" index "-" i)
          :width cell-w :height row-h :padding 0
          :bg :transparent
          (box :width (- cell-w 0.06) :height (- row-h 0.06)
            :corner-radius 1
            :background-color
              (gt/offset-color (nth cells i) (nth measured i))))))))

;; Beat ticks under the map: one per quarter note, so a 16th groove reads as
;; groups of four.
(def heat-ruler (gid slots resolution cell-w)
  (let ((per-beat (max 1 (round (/ 1 resolution)))))
    (h-stack :key (str "rack-groove-heat-ruler-" gid) :gap 0
      (box :width heat-label-width :height 0.5 :bg :transparent)
      (each (range 0 slots) |i|
        (label (if (= (mod i per-beat) 0) (str (+ 1 (floor (/ i per-beat)))) "")
          :width cell-w :height 0.5 :font-size 6
          :color :dim :bg :transparent)))))

(def heatmap (gidx gid state)
  (let ((heat (get state :heatmap)))
    (if (= heat nil)
      (box :key (str "rack-groove-heat-empty-" gid)
        :width (+ heat-label-width heat-width) :height heat-height
        :h-align :center :v-align :center :bg :transparent
        (v-stack :gap 0.3 :align :center
          (label "No groove" :font-size 10 :color :dim :bg :transparent)
          (label "Pick one, or extract the feel of what the rack plays"
            :font-size 7.5 :color :dim :bg :transparent)))
      (let ((rows (get heat :rows))
            (slots (max 1 (get heat :slots))))
        (let ((cell-w (/ heat-width slots))
              (row-h (min 1.1 (/ (- heat-height 0.6) (max 1 (len rows))))))
          (v-stack :key (str "rack-groove-heat-" gid) :debug-name "rack-groove-heatmap"
            :gap 0 :width (+ heat-label-width heat-width)
            (each (range 0 (len rows)) |index|
              (heat-row gidx gid (nth rows index) index cell-w row-h))
            (heat-ruler gid slots (get heat :resolution-beats) cell-w)))))))

;; ── Controls ────────────────────────────────────────────────────────────
(def amount-knob (gid amount label max-value)
  (knob-number :key (str "rack-groove-" amount "-" gid)
    :debug-name (str "rack-groove-" amount)
    :label label
    :value (bind-seq (rack/groove-amount-field amount gid))
    :min 0 :max max-value :decimals 2
    :font-size 8.5 :label-font-size 8
    :text-color :dim :label-color :dim
    :width 4.2 :height 2.6 :knob-size 2.2
    :on-change (lambda (v) (rack/set-groove-amount gid amount v))))

(def small-button (key text on-click)
  (button text :key key
    :width 3.6 :height 0.9 :padding 0 :font-size 7.5
    :background-color '(rgba 0.18 0.2 0.22 1.0)
    :border-color :transparent
    :color :white
    :on-click on-click))

(def active-groove-row (gid state)
  (let ((pool-id (get state :active-groove-id)))
    (if (and (>= pool-id 0) (= groove-renaming pool-id))
      (text-input :key (str "rack-groove-rename-" gid)
        :width 12.8 :height 1.0 :font-size 9
        :value groove-rename-draft
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (name) (set! groove-rename-draft name))
        :on-submit (lambda () (finish-rename gid true))
        :on-cancel (lambda () (finish-rename gid false))
        :on-blur (lambda () (finish-rename gid true)))
      (h-stack :gap 0.3 :align :center :width 12.8
        (label (if (= (get state :active-grid) "") "straight" (get state :active-grid))
          :font-size 7.5 :color :dim :bg :transparent :v-align :center)
        (box :flex 1 :height 0.1 :bg :transparent)
        (if (>= pool-id 0)
          (h-stack :gap 0.2 :align :center
            (small-button (str "rack-groove-rename-button-" gid) "Rename"
              |x y r| (begin-rename state))
            (small-button (str "rack-groove-delete-button-" gid) "Delete"
              |x y r| (rack/delete-groove gid pool-id)))
          (box :width 0 :height 0 :bg :transparent))))))

(def controls (gid state)
  (v-stack :debug-name "rack-groove-controls" :gap 0.35 :width 13 :align :start
    (dropdown :key (str "rack-groove-picker-" gid)
      :debug-name "rack-groove-picker"
      :value (get state :active-label)
      :options (get state :picker-labels)
      :width 12.8 :height 1.0 :font-size 9
      :on-change (lambda (label) (rack/set-groove gid label)))
    (active-groove-row gid state)
    (h-stack :gap 0.1 :align :center
      (amount-knob gid "timing" "Timing" 1.5)
      (amount-knob gid "velocity" "Velocity" 1.5)
      (amount-knob gid "random" "Random" 1.0))
    (button "EXTRACT GROOVE…" :key (str "rack-groove-extract-" gid)
      :debug-name "rack-groove-extract"
      :width 12.8 :height 1.1 :padding 0 :font-size 8
      :background-color '(rgba 0.18 0.22 0.23 1.0)
      :border-color :transparent
      :color :white
      :on-click |x y r| (open-extract gid))
    (label "Member MIDI FX quantizers re-straighten grooved trigs"
      :width 12.8 :font-size 6.5 :color :dim :bg :transparent)))

;; ── Panel ───────────────────────────────────────────────────────────────
(def panel (gidx)
  (let ((gid (rack/group-id gidx))
        (state (or (rack/groove-state gid)
                   (dict :active-key "off" :active-label "Off" :active-groove-id -1
                         :active-grid "" :picker-labels (list "Off")
                         :picker-keys (list "off") :grooves (list) :heatmap nil))))
    (box :debug-name "rack-groove-panel"
      :key (str "rack-groove-panel-" gid)
      :background "fx-panel-bg"
      :color :instrument-panel-bg
      :header :fx-panel-header-bg
      :selected-header :fx-panel-header-selected-bg
      :height st/fx-fixed-panel-height
      :padding 0
      :v-align :start :h-align :start
      (v-stack :gap 0 :height :fill
        (box :width :fill :height 1 :padding 0 :v-align :center :h-align :start
          (h-stack :gap 0.6 :align :center :width :fill
            (pf/fx-panel-header-leading-spacer)
            (label "Groove" :v-align :center :font-size 11 :color :white :bg :transparent)
            (box :flex 1 :height 0.15)
            (label (if (rack/groove-active? gid) "ON" "OFF")
              :font-size 8 :v-align :center
              :color (if (rack/groove-active? gid) :blue :dim) :bg :transparent)
            (box :width 0.5)))
        (pf/fx-panel-body "rack-groove-body"
          (h-stack :gap 0.8 :align :start :padding 0.3
            (controls gid state)
            (heatmap gidx gid state)))))))

;; Mounted once per rack panel; a modal overlays the whole frame, and the
;; *fx* tile is active because its Extract button was just clicked.
(def extract-modal ()
  (modal :is-open extract-open? :on-close (lambda () (close-extract))
      :width-px 760 :height-px 560
    (box :debug-name "rack-groove-extract-panel" :width :fill :height :fill
      :padding 0.6 :bg :transparent
      (if extract-open?
        (v-stack :width :fill :height :fill :gap 0.5
          (label "Extract Groove" :key "rack-groove-extract-title"
            :font-size 16 :color :white :bg :transparent)
          (label "Reads the timing and accents the pads play now."
            :font-size 10 :color :dim :bg :transparent)
          (label "Name" :font-size 10 :color :dim :bg :transparent)
          (text-input :key "rack-groove-extract-name" :width :fill :height 1.3 :font-size 12
            :value extract-name
            :auto-focus true
            :select-all-on-focus true
            :on-change (lambda (v) (set! extract-name v))
            :on-submit (lambda () (commit-extract))
            :on-cancel (lambda () (close-extract)))
          (h-stack :gap 1 :align :center
            (label "Period" :width 5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-bars"
              :value extract-bars :options '("1 bar" "2 bars")
              :width 7 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! extract-bars v)))
            (label "Grid" :width 3.5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-resolution"
              :value extract-resolution :options '("1/16" "1/32")
              :width 6 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! extract-resolution v))))
          (h-stack :gap 0.6 :align :center
            (toggle :key "rack-groove-extract-quantize"
              :value extract-quantize
              :on-change (lambda (v) (set! extract-quantize v)))
            (label "Quantize source afterwards"
              :font-size 10 :color :white :bg :transparent))
          (label (if extract-quantize
                   "The source keeps its sound, now played through the groove."
                   "Only adds it: picking it over its own source doubles the feel.")
            :font-size 9 :color :dim :bg :transparent)
          (box :flex 1 :bg :transparent)
          (h-stack :width :fill :gap 0.5
            (box :flex 1 :bg :transparent)
            (button "Cancel" :key "rack-groove-extract-cancel"
              :on-click |x y r| (close-extract))
            (button "Extract" :key "rack-groove-extract-submit" :variant :primary
              :on-click |x y r| (commit-extract))))
        (box :width 0 :height 0 :bg :transparent)))))
