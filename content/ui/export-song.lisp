;; Command-driven saved-arrangement export. The menu entry will live elsewhere.
(module eseq.export-song)
(export export-song panel open close reset open? name-draft range-draft)

(defstate open? false)
(defstate name-draft "")
(defstate range-draft "Entire arrangement")
(defstate start-draft "0")
(defstate end-draft "0")
(defstate rate-draft "48000")
(defstate tail-draft "10")

(def export-song () (host-command "export-song-open" (dict)))
(def open () (set! open? true))
(def close () (set! open? false))
(def reset ()
  (set! name-draft EXPORT.export-default-name)
  (set! end-draft (str EXPORT.export-end))
  (set! start-draft "0")
  (set! range-draft "Entire arrangement"))

(def start ()
  (host-command "export-song-start"
    (dict :name name-draft :range range-draft :start start-draft
          :end end-draft :sample-rate rate-draft :tail tail-draft)))

(def field (key title value change)
  (v-stack :width :fill :gap 0.2
    (label title :font-size 11 :color :dim :bg :transparent)
    (text-input :key key :width :fill :height 1.1 :font-size 12
      :value value :on-change change)))

(def settings ()
  (v-stack :width :fill :gap 0.35
    (field "export-name" "File name" name-draft (lambda (v) (set! name-draft v)))
    (v-stack :width :fill :gap 0.2
      (label "Range" :font-size 11 :color :dim :bg :transparent)
      (dropdown :key "export-range" :width :fill :height 1.1 :font-size 12
        :value range-draft :options (list "Entire arrangement" "Beat range")
        :on-change (lambda (v) (set! range-draft v))))
    (if (= range-draft "Beat range")
      (h-stack :width :fill :gap 0.35
        (box :flex 1 :bg :transparent
          (field "export-start" "Start beat (from 0)" start-draft (lambda (v) (set! start-draft v))))
        (box :flex 1 :bg :transparent
          (field "export-end" "End beat" end-draft (lambda (v) (set! end-draft v)))))
      (box :height 0 :width 0))
    (h-stack :width :fill :gap 0.35
      (v-stack :flex 1 :gap 0.2
        (label "Sample rate" :font-size 11 :color :dim :bg :transparent)
        (dropdown :key "export-rate" :width :fill :height 1.1 :font-size 12
          :value rate-draft :options (list "44100" "48000" "96000")
          :on-change (lambda (v) (set! rate-draft v))))
      (box :flex 1 :bg :transparent
        (field "export-tail" "Tail (seconds)" tail-draft (lambda (v) (set! tail-draft v)))))
    (label "Stereo WAV · 32-bit float" :font-size 10 :color :dim :bg :transparent)))

(def body ()
  (v-stack :width :fill :height :fill :gap 0.35
    (h-stack :width :fill :align :center
      (label "Export song" :font-size 18 :color :white :bg :transparent)
      (box :flex 1 :bg :transparent)
      (button "×" :key "export-close" :on-click |x y r| (close)))
    (if (not (= EXPORT.export-message ""))
      (label EXPORT.export-message :width :fill :wrap true :key "export-status" :font-size 16 :color :white :bg :transparent)
      (box :width 0 :height 0))
    (if (and EXPORT.export-busy (< EXPORT.export-percent 0))
      (label "Loading instruments and samples…" :key "export-preparing" :font-size 11 :color :dim :bg :transparent)
      (box :width 0 :height 0))
    (scroll :key "export-settings-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.35
        (label (str "Saved project: " EXPORT.export-project) :font-size 12 :color :white :bg :transparent)
        (if (or EXPORT.export-busy EXPORT.export-done)
          (box :width 0 :height 0)
          (label "Exports the last saved version of this project." :key "export-save-note" :font-size 10 :color :dim :bg :transparent))
        (if (or EXPORT.export-busy EXPORT.export-done)
          (label EXPORT.export-output-name :font-size 13 :color :white :bg :transparent)
          (settings))
        (v-stack :width :fill :gap 0.15
          (label "Recordings folder" :font-size 10 :color :dim :bg :transparent)
          (label EXPORT.export-folder :width :fill :wrap true :font-size 9 :color :dim :bg :transparent))
      ))
    (h-stack :width :fill :gap 0.5
      (box :flex 1 :bg :transparent)
      (if EXPORT.export-busy
        (button "Cancel export" :key "export-cancel"
          :on-click |x y r| (host-command "export-song-cancel" (dict)))
        (if EXPORT.export-done
          (h-stack :gap 0.5
            (button "New export" :key "export-new" :on-click |x y r| (export-song))
            (button EXPORT.export-reveal-label :key "export-reveal"
              :on-click |x y r| (host-command "export-song-reveal" (dict))))
          (button "Export" :key "export-submit" :variant :primary :on-click |x y r| (start)))))))

(def panel ()
  (modal :is-open open? :on-close (lambda () (close)) :width-px 720 :height-px 740
    (box :debug-name "export-song-panel" :width :fill :height :fill :padding 0.5 :bg :transparent
      (if open? (body) (box :width 0 :height 0 :bg :transparent)))))
