;; ui/effects/rack-groove.lisp — the drum rack panel's Groove section and its
;; Extract Groove modal (docs/rack-groove-spec.md, "UI" and "Rev 2 UI").
;;
;; Sits beside the pad grid in the *fx* buffer's rack selection panel
;; (effects/buffers.lisp). A groove is an extracted feel in the project pool
;; that the rack plays through. The section is deliberately slim
;; (eseq-groove.12): the picker lists the pool's grooves, then a Library
;; header over the factory/user files (picking one copies it into the pool),
;; then Off; the Timing / Velocity / Random knobs scale it; "Extract
;; Groove…" reads a new one; and a "Grooves tab" link opens the browser's
;; Grooves tab with this rack's groove selected. The heatmap, rename and
;; delete live in that tab (eseq.grooves-tab). The data and every host
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

(def grid-row (gid state)
  (h-stack :gap 0.3 :align :center :width 12.8
    (label (if (= (get state :active-grid) "") "straight" (get state :active-grid))
      :font-size 7.5 :flex 1 :color :dim :bg :transparent :v-align :center)
    ;; The heatmap, rename and delete are in the tab.
    (button "Grooves tab ›" :key (str "rack-groove-tab-link-" gid)
      :debug-name "rack-groove-tab-link"
      :width 5.6 :height 0.9 :padding 0 :font-size 7.5
      :variant :ghost
      :border-color :transparent
      :color :blue
      :on-click |x y r| (gt/show-rack-groove gid))))

(def controls (gid state)
  (v-stack :debug-name "rack-groove-controls" :gap 0.35 :width 13 :align :start
    (dropdown :key (str "rack-groove-picker-" gid)
      :debug-name "rack-groove-picker"
      :value (get state :active-label)
      :options (get state :picker-labels)
      :headers (or (get state :picker-headers) (list))
      :width 12.8 :height 1.0 :font-size 9
      :on-change (lambda (label) (rack/set-groove gid label)))
    (grid-row gid state)
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
    (label "MIDI FX quantizers undo the groove"
      :width 12.8 :font-size 6.5 :color :dim :bg :transparent)))

;; ── Panel ───────────────────────────────────────────────────────────────
(def panel (gidx)
  (let ((gid (rack/group-id gidx))
        (state (or (rack/groove-state gid)
                   (dict :active-key "off" :active-label "Off" :active-groove-id -1
                         :active-grid "" :picker-labels (list "Off")
                         :picker-keys (list "off") :picker-headers (list)
                         :grooves (list)))))
    (box :debug-name "rack-groove-panel"
      :key (str "rack-groove-panel-" gid)
      :background "fx-panel-bg"
      :color :instrument-panel-bg
      :header :fx-panel-header-bg
      :selected-header :fx-panel-header-selected-bg
      :width 14
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
          (box :padding 0.3 :bg :transparent
            (controls gid state)))))))

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
