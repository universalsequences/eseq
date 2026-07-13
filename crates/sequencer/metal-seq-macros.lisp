;; Reusable project-macro controls for script-authored player surfaces.
;; This file intentionally mounts no buffer of its own.

(defstate macro-mapping-open false)
(defstate macro-mapping-selected -1)

;; Script keys are written in canonical lowercase. `str` preserves strings and
;; prefixes keywords with `:`, so normalize both accepted spellings for lookup.
;; The command layer performs the engine's full validation/canonicalization too.
(def macro-key-string (key)
  (let ((text (str key)))
    (if (= key text) text (substring text 1))))

(def macro-by-key (key)
  (nth
    (filter |macro| (= (get macro :key) (macro-key-string key)) SEQ.macros)
    0))

(def macro-id-for-key (key)
  (let ((macro (macro-by-key key)))
    (if macro (get macro :id) -1)))

(def macro-ensure (key name)
  (host-command "macro-ensure"
    (dict :key (macro-key-string key) :name name)))

(def macro-set-key-value (key value)
  (let ((id (macro-id-for-key key)))
    (if (>= id 0)
      (host-command "macro-set-value" (dict :id id :value value))
      false)))

(def macro-mapping-active-for-key? (key)
  (let ((id (macro-id-for-key key)))
    (and macro-mapping-open (>= id 0) (= macro-mapping-selected id))))

(def macro-clear-mapping-arm ()
  (do
    (set! macro-mapping-open false)
    (set! macro-mapping-selected -1)))

(def macro-toggle-mapping-arm (key)
  (let ((id (macro-id-for-key key)))
    (if (< id 0)
      false
      (if (macro-mapping-active-for-key? key)
        (macro-clear-mapping-arm)
        (do
          (set! macro-mapping-open true)
          (set! macro-mapping-selected id))))))

;; Usage: (macro-knob :macro :delay-push)
(def macro-knob (_macro key)
  (let ((macro (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-knob-" resolved-key)
      (knob-number
        :label (if macro (get macro :name) resolved-key)
        :value (if macro (get macro :value) 0)
        :min 0 :max 1 :decimals 2
        :width 7.0 :height 4.2 :knob-size 2.2
        :font-size 9.0 :label-font-size 9.0
        :label-color :dim
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (value) (macro-set-key-value key value))))))

;; Usage: (macro-map-button :macro :delay-push)
(def macro-map-button (_macro key)
  (let ((active (macro-mapping-active-for-key? key)))
    (button (if active "mapping..." "map")
      :width 7.0 :height 1.25 :font-size 9.0
      :background-color (if active (rgba 0.27 0.78 0.43 1.0) :mixer-control-bg)
      :color (if active :black :dim)
      :on-click (lambda (event) (macro-toggle-mapping-arm key)))))
