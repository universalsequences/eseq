;; Command-driven saved-arrangement export. The menu entry will live elsewhere.
;; The job's state is the host's `song-export` singleton; the open flag and
;; the settings being drafted are this view's own (`export-draft`).
(module eseq.export-song)
(import eseq.kinds :refer (song-export))
(export export-song panel open close reset export-draft)

;; The modal and its settings, as typed (the host parses them on Export).
(def-kind export-draft
  :key ()
  :state ((open false)
          (name "")
          (range "Entire arrangement")
          (start "0")
          (end "0")
          (rate "48000")
          (tail "10")))

(def export-song () (host-command "export-song-open" (dict)))
(def open () (set! export-draft.open true))
(def close () (set! export-draft.open false))
;; Fresh settings for a new export: the suggested file name and the
;; arrangement's end beat. The host calls this right after presenting the
;; export, before its next tick pushes `song-export`, so it passes them.
(def reset (default-name end-beat)
  (set! export-draft.name default-name)
  (set! export-draft.end (str end-beat))
  (set! export-draft.start "0")
  (set! export-draft.range "Entire arrangement"))

(def start ()
  (host-command "export-song-start"
    (dict :name export-draft.name :range export-draft.range :start export-draft.start
          :end export-draft.end :sample-rate export-draft.rate :tail export-draft.tail)))

(def field (key title value change)
  (v-stack :width :fill :gap 0.2
    (label title :font-size 11 :color :dim :bg :transparent)
    (text-input :key key :width :fill :height 1.1 :font-size 12
      :value value :on-change change)))

(def settings ()
  (v-stack :width :fill :gap 0.35
    (field "export-name" "File name" export-draft.name (lambda (v) (set! export-draft.name v)))
    (v-stack :width :fill :gap 0.2
      (label "Range" :font-size 11 :color :dim :bg :transparent)
      (dropdown :key "export-range" :width :fill :height 1.1 :font-size 12
        :value export-draft.range :options (list "Entire arrangement" "Beat range")
        :on-change (lambda (v) (set! export-draft.range v))))
    (if (= export-draft.range "Beat range")
      (h-stack :width :fill :gap 0.35
        (box :flex 1 :bg :transparent
          (field "export-start" "Start beat (from 0)" export-draft.start (lambda (v) (set! export-draft.start v))))
        (box :flex 1 :bg :transparent
          (field "export-end" "End beat" export-draft.end (lambda (v) (set! export-draft.end v)))))
      (box :height 0 :width 0))
    (h-stack :width :fill :gap 0.35
      (v-stack :flex 1 :gap 0.2
        (label "Sample rate" :font-size 11 :color :dim :bg :transparent)
        (dropdown :key "export-rate" :width :fill :height 1.1 :font-size 12
          :value export-draft.rate :options (list "44100" "48000" "96000")
          :on-change (lambda (v) (set! export-draft.rate v))))
      (box :flex 1 :bg :transparent
        (field "export-tail" "Tail (seconds)" export-draft.tail (lambda (v) (set! export-draft.tail v)))))
    (label "Stereo WAV · 32-bit float" :font-size 10 :color :dim :bg :transparent)))

(def body ()
  (v-stack :width :fill :height :fill :gap 0.35
    (h-stack :width :fill :align :center
      (label "Export song" :font-size 18 :color :white :bg :transparent)
      (box :flex 1 :bg :transparent)
      (button "×" :key "export-close" :on-click |x y r| (close)))
    (if (not (= song-export.message ""))
      (label song-export.message :width :fill :wrap true :key "export-status" :font-size 16 :color :white :bg :transparent)
      (box :width 0 :height 0))
    (if (and song-export.busy (< song-export.percent 0))
      (label "Loading instruments and samples…" :key "export-preparing" :font-size 11 :color :dim :bg :transparent)
      (box :width 0 :height 0))
    (scroll :key "export-settings-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.35
        (label (str "Saved project: " song-export.project) :font-size 12 :color :white :bg :transparent)
        (if (or song-export.busy song-export.done)
          (box :width 0 :height 0)
          (label "Exports the last saved version of this project." :key "export-save-note" :font-size 10 :color :dim :bg :transparent))
        (if (or song-export.busy song-export.done)
          (label song-export.output-name :font-size 13 :color :white :bg :transparent)
          (settings))
        (v-stack :width :fill :gap 0.15
          (label "Recordings folder" :font-size 10 :color :dim :bg :transparent)
          (label song-export.folder :width :fill :wrap true :font-size 9 :color :dim :bg :transparent))
      ))
    (h-stack :width :fill :gap 0.5
      (box :flex 1 :bg :transparent)
      (if song-export.busy
        (button "Cancel export" :key "export-cancel"
          :on-click |x y r| (host-command "export-song-cancel" (dict)))
        (if song-export.done
          (h-stack :gap 0.5
            (button "New export" :key "export-new" :on-click |x y r| (export-song))
            (button song-export.reveal-label :key "export-reveal"
              :on-click |x y r| (host-command "export-song-reveal" (dict))))
          (button "Export" :key "export-submit" :variant :primary :on-click |x y r| (start)))))))

(def panel ()
  (modal :is-open export-draft.open :on-close (lambda () (close)) :width-px 720 :height-px 740
    (box :debug-name "export-song-panel" :width :fill :height :fill :padding 0.5 :bg :transparent
      (if export-draft.open (body) (box :width 0 :height 0 :bg :transparent)))))
