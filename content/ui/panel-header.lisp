;; ui/panel-header.lisp — the right column's panel header: a rounded dim
;; pill holding a filled chip (the *step* panel's track badge, the
;; *processes* dock's instance name) and muted secondary text. One home so
;; *step* and the dock stay identical (eseq-waa9.22).
;;
;; No imports: a leaf both eseq.effects.track-panels and
;; eseq.processes-buffer import.
(module eseq.panel-header)

(export pill chip note)

;; The pill: `chip` then `summary`, on the selected-strip background.
(def pill (chip summary)
  (box :padding 0.25 :background-color :mixer-strip-selected-bg :corner-radius 12 :v-align :center
    (h-stack :gap 0.45 :align :start
      chip
      summary)))

;; The filled chip: `text` in black on `fill` (dimmed to `muted-fill` with
;; dim text while `muted`).
(def chip (key text width fill muted muted-fill)
  (box
    :key key
    :width width :height 1.0
    :padding 0
    :corner-radius 8
    :v-align :center
    :muted muted
    :background-color fill
    :muted-background-color muted-fill
    (label text
      :width width
      :font-size 10
      :v-align :center
      :h-align :center
      :active muted
      :color :black
      :active-color :dim
      :bg :transparent)))

;; Muted secondary text beside the chip.
(def note (key text)
  (label text :key key :font-size 8 :color :dim :bg :transparent))
