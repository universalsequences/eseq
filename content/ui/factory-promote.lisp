;; Promote to factory (eseq-jhmx): name a Sound, kit or preset and copy it
;; into the checkout's content/ tree so it ships with the app. Opened by the
;; M-x commands in main.lisp (`promote-sound-to-factory`,
;; `promote-kit-to-factory`, `promote-preset-to-factory`); Rust
;; (src/ui/host_commands/factory_promote.rs) captures the target, vets every
;; dependency, and presents what will be skipped in the `factory-promote`
;; singleton. The open flag and the name being typed are this view's own
;; (`promote-form`).
;;
;; Mounted by both step-panel buffers, like Resample; Rust activates the
;; mount's tile before calling `open`.
(module eseq.factory-promote)
(import eseq.kinds :refer (factory-promote))
(export panel open close commit promote-form)

(def-kind promote-form
  :key ()
  :state ((open false)
          (name "")))

;; Open on `default-name`, the name the host suggests.
(def open (default-name)
  (set! promote-form.name default-name)
  (set! promote-form.open true))

(def close ()
  (set! promote-form.open false))

(def blocked? ()
  (not (= factory-promote.blocking "")))

;; A second Promote after "already exists" replaces the factory copy.
(def replacing? ()
  (and (not (= factory-promote.taken ""))
       (= (string-downcase factory-promote.taken) (string-downcase promote-form.name))))

(def commit ()
  (unless (blocked?)
    (host-command "factory-promote-commit"
      (dict :name promote-form.name :overwrite (replacing?)))))

(def skipped-row (i line)
  (label (str "- " line)
    :key (str "skip-" i)
    :font-size 10.5 :color :yellow :bg :transparent))

(def body ()
  (let ((skipped factory-promote.skipped))
    (v-stack :width :fill :height :fill :gap 0.6
      (label (str "Promote " factory-promote.target " to factory")
        :key "title" :bg :transparent :font-size 16)
      (if (blocked?)
        (label factory-promote.blocking
          :key "blocking" :bg :transparent :font-size 11 :color :red)
        (label (str "Writes to " factory-promote.destination
                    " so it ships with the app. Commit the file to keep it.")
          :key "destination" :bg :transparent :font-size 10 :color :dim))
      (v-stack :width :fill :gap 0.3
        (label "Name" :bg :transparent :font-size 11)
        (text-input :key "name" :width :fill :height 1.35 :font-size 11
          :value promote-form.name :placeholder "factory name"
          :on-change (lambda (v) (set! promote-form.name v))
          :on-submit (lambda () (commit))))
      (if (> (len skipped) 0)
        (v-stack :width :fill :gap 0.2 :flex 1
          (label (str "Not factory content, will be skipped ("
                      (len skipped) "):")
            :key "skipped-title" :bg :transparent :font-size 11)
          (scroll :key "skipped-scroll" :width :fill :flex 1
            (v-stack :width :fill :gap 0.15
              (each (range 0 (len skipped)) |i|
                (skipped-row i (nth skipped i))))))
        (if (blocked?)
          (box :height 0 :flex 1)
          (label "Every dependency is factory content; it ships whole."
            :key "all-factory" :bg :transparent :font-size 10.5 :color :green :flex 1)))
      (label factory-promote.error :key "error" :bg :transparent :font-size 11 :color :red)
      (h-stack :width :fill :gap 0.8 :align :center
        (box :height 0 :flex 1)
        (button "Cancel" :key "cancel" :variant :ghost :on-click |event| (close))
        (button (if (replacing?) "Replace factory copy" "Promote")
          :key "commit"
          :disabled (blocked?)
          :on-click |event| (commit))))))

(def panel ()
  (modal :key "factory-promote-modal" :is-open promote-form.open :on-close (lambda () (close))
    :title "Promote to factory" :width-px 760 :height-px 520
    ;; Closed, the step panel's rebuilds must not evaluate the body.
    (if promote-form.open (body) (box :height 0 :width 0))))
