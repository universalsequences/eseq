;; ui/preview-strip.lisp — The sample preview strip the browser and the
;; sample import modal share (Ableton-style: headphone toggle + waveform).
;;
;; Each caller owns a preview singleton `p` with `path` (the sample the
;; cursor landed on, "" for none), `buffer` (its waveform-buffer map from
;; `seq-sample-waveform`, false while nothing decodable is focused) and
;; `auto` (the headphone): `eseq.browser/sample-preview`,
;; `eseq.sample-import/import-preview`. These take the singleton so both
;; strips share one behaviour. A module of its own (not ui/browser.lisp) so
;; the import modal does not load the whole browser.

(module eseq.preview-strip)

(import eseq.kinds :refer (browser))

(export stop-preview sync-preview! toggle-preview! preview-strip)

;; Headphone toggle for the preview strip (Ableton-style auto-preview): a
;; circular badge with the headphone glyph inside — headband arc (ring clipped
;; to the top half) meeting two ear-cup capsules. Badge fills blue while
;; auto-preview is armed.
(defwidget preview-headphone-icon
  :width 2.8 :height 1.8
  :paint-margin 0.3
  :state (active)
  :shader
  (let ((badge-col (if (= active 1) :accent (rgba 0.33 0.34 0.36 1.0)))
        (glyph-col (if (= active 1) :bg (rgba 0.66 0.68 0.72 1.0)))
        (band (max (- (abs (- (sqrt (+ (* x x) (* y y))) 0.36)) 0.06) y)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.72) (material :color badge-col))
      (sdf/fill band (material :color glyph-col))
      (sdf/fill (sdf/translate -0.36 0.12 (sdf/rounded-rect 0.14 0.34 0.11))
        (material :color glyph-col))
      (sdf/fill (sdf/translate 0.36 0.12 (sdf/rounded-rect 0.14 0.34 0.11))
        (material :color glyph-col)))))

(def stop-preview ()
  (if browser.preview-playing
    (host-command "stop-sample-preview" (dict))
    nil))

;; p now shows `path` ("" for none). With the headphone on, landing on a
;; sample plays it once — the host player replaces any preview still in
;; flight, so no explicit stop is needed. With it off, just silence whatever
;; was still sounding.
(def sync-preview! (p path)
  (if (= path p.path) nil
    (do
      (set! p.path path)
      (set! p.buffer (if (= path "") false (seq-sample-waveform path)))
      (if (and p.auto p.buffer)
        (host-command "preview-sample" (dict :path path))
        (stop-preview)))))

;; Flips p's headphone, returning it. Turning it on immediately previews the
;; focused sample.
(def toggle-preview! (p)
  (if p.auto
    (do
      (set! p.auto false)
      (stop-preview))
    (do
      (set! p.auto true)
      (if p.buffer
        (host-command "preview-sample" (dict :path p.path))
        nil)))
  p.auto)

;; p's strip: headphone toggle + waveform with the shared preview playhead.
;; `prefix` starts its keys, `bg` fills it, `empty` stands in while p has no
;; waveform, and `on-toggle` runs on a headphone click.
(def preview-strip (p prefix bg empty on-toggle)
  (if p.buffer
    (box :key (str prefix "preview-strip") :width :fill :height 1.5
      :background-color bg :corner-radius 8 :padding 0.03
      (h-stack :width :fill :gap 0.35 :align :baseline
        (box :key (str prefix "preview-headphone") :width 2.3 :height 2.2 :align :center
          :on-click |x y r| (on-toggle)
          (preview-headphone-icon :active (if p.auto 1 0)))
        (box :width 0 :flex 1 :height 2.3
          (subtree :key (str prefix "preview-wave-" p.path)
            (waveform
              :height 2
              :header-height 0
              :bg bg
              :waveform-color :dim
              :grid-major-color :transparent
              :grid-minor-color :transparent
              :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
              :view-start 0
              :view-duration (get p.buffer :duration)
              :selection-start 0
              :selection-end (get p.buffer :duration)
              :playhead-time #'browser.preview-position
              :buffer p.buffer)))))
    empty))
