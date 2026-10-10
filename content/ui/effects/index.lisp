;; eseq.effects — one import for every module factory instrument/effect UIs
;; call by qualified name (eseq.effects.custom-ui-lego/…, drum-surface/…, …).
;;
;; `(import eseq.effects)` resolves here: a module name whose path is a
;; directory loads that directory's index.lisp (kind-bindings spec §11). A
;; bare `-noui` view needs only this import to render factory device panels.
;; The app's own ui/effects.lisp manifest still `load`s these files by path;
;; import is load-once per pass, so a module already `load`ed is not re-run.

(module eseq.effects)

(import eseq.effects.state)
(import eseq.effects.param-controls)
(import eseq.effects.custom-ui-runtime)
(import eseq.effects.custom-ui-sections)
(import eseq.effects.custom-ui-controls)
(import eseq.effects.custom-ui-lego)
(import eseq.effects.custom-effect-ui)
(import eseq.effects.mnm-surface)
(import eseq.effects.drum-surface)
(import eseq.effects.physical-model-surface)
(import eseq.effects.identified-drum)
(import eseq.effects.panel-bodies)

;; Rack slot clicks select the slot as the factory rack panel does.
(import eseq.effects.instrument-panel)
(import eseq.effects.sampler-panel :as sp)
(import eseq.effects.modulator-panel :as mp)
(import eseq.effects.panel-data :as pd)

(export device-panel device-panel-body panel-height panel-buffer rack-slot-select)

;; The factory's device panel buffer: a view showing the factory panels
;; names its tile "*fx*". (`effect-buffer` takes a literal name: write
;; "*fx*" there and use `panel-buffer` in layouts.)
(def panel-buffer "*fx*")

;; Height of a panel body, the factory's fixed device-panel height less its
;; header and padding: what `device-panel-body` lays out at.
(def panel-height eseq.effects.state/fx-panel-body-content-height)

;; The panel data of `d`, an eseq.kinds device (`(nth t.devices i)`,
;; `(nth t.midi-devices i)`), built from the kinds (eseq.effects.panel-data):
;; the instrument panel for the instrument (slot -1; its `:type` is "rack"
;; for a rack, with `:slots`, `:selected-slot` and `:selected-instrument`),
;; else the effect's (`:params` holds its parameters). Nil while d's track
;; is not the selected one: the panels' controls edit the selected track.
(def device-panel (d)
  (if (and d d.track d.track.selected)
    (if (< d.slot 0)
      (pd/instrument-panel-of d.track d)
      (pd/fx-panel-of d))
    nil))

;; An instrument's body, picked by its type as the factory instrument panel
;; picks its panel (eseq.effects.instrument-panel/instrument-panel): the
;; sampler's and modulator's own bodies, else the synth UI.
(def instrument-body (inst)
  (match (get inst :type)
    "sampler" (sp/sampler-panel-content inst)
    "modulator" (mp/modulator-panel-body inst)
    _ (eseq.effects.panel-bodies/instrument-synth-panel-body inst)))

;; The factory body of d's panel (no header), `panel-height` tall: the
;; instrument body of an instrument or of a rack's selected slot, an
;; effect's controls (MIDI or audio, as the factory effect panel picks). Nil
;; when `device-panel` is.
(def device-panel-body (d)
  (let ((panel (device-panel d)))
    (if (= panel nil)
      nil
      (box :height panel-height
        (if (>= d.slot 0)
          (if (get panel :midi-fx)
            (eseq.effects.panel-bodies/midi-fx-panel-body panel)
            (eseq.effects.panel-bodies/audio-fx-panel-body panel (get panel :params)))
          (if (= (get panel :type) "rack")
            (if (get panel :selected-instrument)
              (instrument-body (get panel :selected-instrument))
              (box :width 30 :h-align :center :v-align :center
                (label "Empty slot" :color :dim :bg :transparent)))
            (instrument-body panel)))))))

;; Select a rack slot (an entry of a rack panel's `:slots`).
(def rack-slot-select (slot) (eseq.effects.instrument-panel/rack-slot-select slot))
