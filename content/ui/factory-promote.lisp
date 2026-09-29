;; Promote to factory (eseq-jhmx): name a Sound, kit or preset and copy it
;; into the checkout's content/ tree so it ships with the app. Opened by the
;; M-x commands in main.lisp (`promote-sound-to-factory`,
;; `promote-kit-to-factory`, `promote-preset-to-factory`); Rust
;; (src/ui/host_commands/factory_promote.rs) captures the target, vets every
;; dependency, and publishes what will be skipped in FACTORY_PROMOTE.
;;
;; Mounted by both step-panel buffers, like Resample; Rust activates the
;; mount's tile before calling `open`.
(module eseq.factory-promote)
(export panel open close open? name commit)

(defstate open? false)
(defstate name "")

(def open (default-name)
  (set! name default-name)
  (set! open? true))

(def close ()
  (set! open? false))

;; State fields read nil before the host first publishes them.
(def present? (text)
  (if (and text (not (= text ""))) true false))

(def blocked? ()
  (present? FACTORY_PROMOTE.blocking))

;; A second Promote after "already exists" replaces the factory copy.
(def replacing? ()
  (if (and (present? FACTORY_PROMOTE.taken)
           (= (string-downcase FACTORY_PROMOTE.taken) (string-downcase name)))
    true false))

(def commit ()
  (if (blocked?) nil
    (host-command "factory-promote-commit"
      (dict :name name :overwrite (replacing?)))))

(def skipped-row (i line)
  (label (str "- " line)
    :key (str "skip-" i)
    :font-size 10.5 :color :yellow :bg :transparent))

(def body ()
  (v-stack :width :fill :height :fill :gap 0.6
    (label (str "Promote " FACTORY_PROMOTE.kind " to factory")
      :key "title" :bg :transparent :font-size 16)
    (if (blocked?)
      (label FACTORY_PROMOTE.blocking
        :key "blocking" :bg :transparent :font-size 11 :color :red)
      (label (str "Writes to " FACTORY_PROMOTE.destination
                  " so it ships with the app. Commit the file to keep it.")
        :key "destination" :bg :transparent :font-size 10 :color :dim))
    (v-stack :width :fill :gap 0.3
      (label "Name" :bg :transparent :font-size 11)
      (text-input :key "name" :width :fill :height 1.35 :font-size 11
        :value name :placeholder "factory name"
        :on-change (lambda (v) (set! name v))
        :on-submit (lambda () (commit))))
    (if (and FACTORY_PROMOTE.skipped (> (len FACTORY_PROMOTE.skipped) 0))
      (v-stack :width :fill :gap 0.2 :flex 1
        (label (str "Not factory content, will be skipped ("
                    (len FACTORY_PROMOTE.skipped) "):")
          :key "skipped-title" :bg :transparent :font-size 11)
        (scroll :key "skipped-scroll" :width :fill :flex 1
          (v-stack :width :fill :gap 0.15
            (each (range 0 (len FACTORY_PROMOTE.skipped)) |i|
              (skipped-row i (nth FACTORY_PROMOTE.skipped i))))))
      (if (blocked?)
        (box :height 0 :flex 1)
        (label "Every dependency is factory content; it ships whole."
          :key "all-factory" :bg :transparent :font-size 10.5 :color :green :flex 1)))
    (label FACTORY_PROMOTE.error :key "error" :bg :transparent :font-size 11 :color :red)
    (h-stack :width :fill :gap 0.8 :align :center
      (box :height 0 :flex 1)
      (button "Cancel" :key "cancel" :variant :ghost :on-click |event| (close))
      (button (if (replacing?) "Replace factory copy" "Promote")
        :key "commit"
        :disabled (blocked?)
        :on-click |event| (commit)))))

(def panel ()
  (modal :key "factory-promote-modal" :is-open open? :on-close (lambda () (close))
    :title "Promote to factory" :width-px 760 :height-px 520
    ;; Closed, the step panel's rebuilds must not evaluate the body.
    (if open? (body) (box :height 0 :width 0))))
