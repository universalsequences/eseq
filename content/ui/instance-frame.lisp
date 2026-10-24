;; ui/instance-frame.lisp — The chrome around every kind instance's view
;; (docs/rig-spec.md §10).
;;
;; A project-owned sequencer (density, jaki, a graph kind…) plays in a scene
;; unless the scene switches it off. Its view tab gets the bank-scoped power
;; toggle a drum rack's header has (§6, eseq.bank-power): on/off for every
;; scene of the VIEWED bank, a small dot beside it while the bank is mixed,
;; and the hint after it ("On in bank C") standing in for a tooltip.
;;
;; Registered at load with (set-instance-view-frame! instance-view-frame):
;; the VM then renders every instance view as (instance-view-frame view
;; self). The frame is always the same two children: the power strip inside
;; its own subtree (zero size when it does not show: a rack-owned instance
;; follows its kit, a one-bank project that switches it off nowhere needs
;; none), then the view. Every read of project.instances and of the banks
;; happens inside that subtree, so a bank or instance change re-renders the
;; strip alone, never the whole view.

(module eseq.instance-frame)
(import eseq.kinds :refer (banks instance-row set-instance-parked!))
(import eseq.scene-banks :refer (scene-viewed-bank scene-viewed-bank-label))
(import eseq.bank-power :refer (bank-power-toggle bank-power-hint instance-bank-state))

(export instance-view-frame
        instance-frame-shown?
        instance-viewed-state
        instance-power-menu-label
        set-instance-in-viewed-bank!
        toggle-instance-in-viewed-bank!)

;; Whether `row` (a project.instances row) gets the power strip: a
;; project-owned instance in a project with more than one bank, or one some
;; scene switches off (the way back on is always visible).
(def instance-frame-shown? (row)
  (and row
       (= (get row :owner-rack) nil)
       (or (> (len (banks)) 1)
           (> (len (get row :parked-banks)) 0)
           (> (len (get row :mixed-banks)) 0))))

;; Instance id in the viewed bank: :on, :off or :mixed.
(def instance-viewed-state (id)
  (instance-bank-state id (scene-viewed-bank)))

;; The context-menu entry: "Turn off in bank C" while it plays in any scene
;; of the viewed bank, "Turn on in bank C" while every scene has it off.
(def instance-power-menu-label (id)
  (str "Turn " (if (= (instance-viewed-state id) :off) "on" "off")
       " in bank " (scene-viewed-bank-label)))

;; Switch instance id on (`on` true) or off in every scene of the viewed bank.
(def set-instance-in-viewed-bank! (id on)
  (set-instance-parked! id (scene-viewed-bank) (not on)))

;; The power toggle: on or mixed turns the whole bank off; off turns it on.
(def toggle-instance-in-viewed-bank! (id)
  (set-instance-in-viewed-bank! id (= (instance-viewed-state id) :off)))

;; The strip, or a zero-size box when instance id gets none.
(def instance-power-strip (id)
  (if (instance-frame-shown? (instance-row id))
    (let ((b (scene-viewed-bank))
          (bank-state (instance-bank-state id b)))
      (h-stack :debug-name "instance-power" :width :fill :height 1.2 :gap 0.4 :align :center
        (bank-power-toggle "instance-power-dot" bank-state
          (lambda () (toggle-instance-in-viewed-bank! id)))
        (label (bank-power-hint nil bank-state (if b b.label ""))
          :debug-name "instance-power-hint"
          :v-align :center
          :font-size 9 :color :dim :bg :transparent)
        (box :flex 1 :height 0.15)))
    (box :width 0 :height 0)))

;; Every instance view renders through here (see the header).
(def instance-view-frame (view self)
  (let ((id self.id))
    (v-stack :width :fill :height :fill :gap 0
      (subtree :key "instance-power"
        (instance-power-strip id))
      (box :width :fill :flex 1 :padding 0
        (view self)))))

(set-instance-view-frame! instance-view-frame)
