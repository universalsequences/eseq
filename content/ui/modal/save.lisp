;; eseq.modal.save — "Save instrument" for a modal patch draft track. The
;; draft's instrument panel header shows the button in place of the preset
;; save icon (a draft has no saved instrument to hold presets); it asks the
;; host (`modal-draft-save-open`, src/ui/host_commands/modal_drafts.rs) to
;; activate this mount's tile and `open` the form on the patch's :name.
;; Saving writes the running engine's dsp.lisp / ui.lisp / instrument.json
;; into the user instrument folder; the track stays the script's draft.
;;
;; Mounted by eseq.file-dialogs next to the other dialogs.
(module eseq.modal.save)
(export panel open close commit fail save-form)

(def-kind save-form
  :key ()
  :state ((open false)
          (track 0)
          (name "")
          ;; The name the host found already saved: Save becomes Replace.
          (taken "")
          (error "")))

(def open (track default-name)
  (set! save-form.track track)
  (set! save-form.name default-name)
  (set! save-form.taken "")
  (set! save-form.error "")
  (set! save-form.open true))

(def close ()
  (set! save-form.open false))

;; The host's refusal: `taken` is the name when it is already saved.
(def fail (error taken)
  (set! save-form.error error)
  (set! save-form.taken taken))

(def replacing? ()
  (and (not (= save-form.taken "")) (= save-form.taken save-form.name)))

(def commit ()
  (host-command "modal-draft-save"
    (dict :track save-form.track :name save-form.name :overwrite (replacing?))))

(def body ()
  (v-stack :width :fill :height :fill :gap 0.6
    (label "Save instrument" :key "title" :bg :transparent :font-size 16)
    (label "Saves the draft's generated instrument to your library. The track stays the script's draft."
      :key "hint" :bg :transparent :font-size 10 :color :dim)
    (v-stack :width :fill :gap 0.3
      (label "Name" :bg :transparent :font-size 11)
      (text-input :key "name" :width :fill :height 1.35 :font-size 11
        :value save-form.name :placeholder "instrument name"
        :on-change (lambda (v) (set! save-form.name v))
        :on-submit (lambda () (commit))))
    (label save-form.error :key "error" :bg :transparent :font-size 11 :color :red)
    (box :height 0 :flex 1)
    (h-stack :width :fill :gap 0.8 :align :center
      (box :height 0 :flex 1)
      (button "Cancel" :key "cancel" :variant :ghost :on-click |event| (close))
      (button (if (replacing?) "Replace" "Save")
        :key "commit"
        :disabled (= (string-trim save-form.name) "")
        :on-click |event| (commit)))))

(def panel ()
  (modal :key "modal-save-modal" :is-open save-form.open :on-close (lambda () (close))
    :title "Save instrument" :width-px 520 :height-px 300
    (if save-form.open (body) (box :height 0 :width 0))))
