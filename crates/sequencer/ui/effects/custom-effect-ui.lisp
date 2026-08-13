;; Custom MIDI/audio effect parameter lookup helpers.
(module eseq.effects.custom-effect-ui)

(import eseq.effects.param-grid :as pg)
(import eseq.effects.custom-ui-runtime :as rt)

;; Migration aliases (module spec §10). All four are identity aliases:
;; this file is part of the generated-custom-UI vocabulary (hub-file
;; precedent). Flat callers that cannot see qualified names:
;; - src/ui/custom_ui.rs emits `(midi-fx-ui-param-control …)` /
;;   `(audio-fx-ui-param-control …)` calls into implicit-module units
;;   (custom_ui.rs:154/167/196/209);
;; - generated per-effect ui.lisp files on disk reference
;;   `audio-fx-ui-param` / `midi-fx-ui-param` directly
;;   (crates/sequencer/effects/**/ui.lisp, midi-fx/**/ui.lisp);
;; - src/agent/ui_validate.rs's stub table proves generated effect UIs
;;   may call `audio-fx-ui-param` by flat spelling.
;; The standalone Runtime::new() harnesses in state_values/tests.rs and
;; ui_validate.rs define their OWN stubs of these names and never load
;; this file — they need no alias (the mixer track-peak precedent).
(module-compat-alias midi-fx-ui-param midi-fx-ui-param)
(module-compat-alias midi-fx-ui-param-control midi-fx-ui-param-control)
(module-compat-alias audio-fx-ui-param audio-fx-ui-param)
(module-compat-alias audio-fx-ui-param-control audio-fx-ui-param-control)

;; The current-fx globals below are pinned to eseq.vanilla by their owner
;; (effects/state.lisp, spec §10 hazard i): src/ui/custom_ui.rs GENERATES
;; lisp that `set!`s them by bare name (custom_ui.rs:582/682). They are
;; mutable plain defs, so a bare read here would freeze on first heal
;; (hazard m) — every read uses the qualified `eseq.vanilla/` spelling,
;; which reduces to the flat slot the codegen writes (the
;; custom-ui-runtime precedent).
;;
;; The `"custom-midi-fx-ui-…"`/`"custom-audio-fx-ui-…"` strings below are
;; subtree keys (pg/fx-param-row wraps its third argument in
;; `(subtree :key …)`) — byte-identical keyspace, never qualified.

(def midi-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def midi-fx-ui-param-control (name)
  (let ((p (midi-fx-ui-param eseq.vanilla/midi-fx-ui-current-fx name)))
    (if p
      (pg/fx-param-row p eseq.vanilla/midi-fx-ui-current-fx
        (str "custom-midi-fx-ui-" eseq.vanilla/midi-fx-ui-current-name
             "-slot-" (get eseq.vanilla/midi-fx-ui-current-fx :slot-idx) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def audio-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def audio-fx-ui-param-control (name)
  (let ((p (audio-fx-ui-param eseq.vanilla/audio-fx-ui-current-fx name)))
    (if p
      (pg/fx-param-row p eseq.vanilla/audio-fx-ui-current-fx
        (str "custom-audio-fx-ui-" (rt/custom-ui-scope-name) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
