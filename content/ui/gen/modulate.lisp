;; eseq.gen.modulate — making a generated instrument's knobs modulatable
;; (docs/program-generators-spec.md §5.2), as a pass over its forms.
;;
;; Range params are modulatable: the host adds its sources (mod-sources),
;; scaled by per-param depths, to the knob, read as (mod p) and gated off
;; while a param has no route. A knob is constant within a block, so DGen
;; works out what depends only on knobs (setup: a plate's modes from its
;; size, a point's shape on it) once per block; read modulated, that work
;; would run every sample. So by default only knobs no setup reads are
;; modulatable, free until modulated: per-sample code reads (mod p), and
;; the work on an event clock (a by-note's, held coefficients') reads
;; (event-hold (mod p) clock), the clock itself watching the sources every
;; mod-hold samples (change-tick-forms). With all (eseq.modal: a patch's
;; :modulate :all), the knobs setup reads are modulatable too, held: sampled
;; on mod_tick,
;; p_modh feeds the setup, which runs on that tick only, p_mod keeps it in a
;; history for everything else, and each setup value other code reads is
;; latched as <name>_mod, a cost every sample, modulated or not
;; (eseq-k0m2.47: the cello +9%, the piano +80%, the Ride Kit +110%).

(module eseq.gen.modulate)

(import eseq.gen :refer (concat-all member? any? head? head-of-3? def-form? form-atoms def-names map-atoms
  rename-in defines? change-tick-forms))

(export mod-pass)

;; The heads of DGenLisp forms that keep state or read the voice's inputs: a
;; def using one runs every sample, and so does anything that reads it (the
;; instrument's own defmacros count too, and the library defmacros mod-pass
;; is told keep state: see setup-names).
(def per-sample-heads
  '(in read-history write-history make-history latch event-hold svf adsr noise delay phasor accum))

;; Voice inputs that change only between notes (or under a glide): DGen
;; works out what depends only on them and knobs once per block too, so
;; setup-names treats them, and what a note's start latches (slow-def?), as
;; not per-sample; mod_tick watches them.
(def slow-names '(f0))

;; True when a def is of a slow value: one of slow-names, a latch (what a
;; note's start latches, the played key and its velocity; a held object's
;; coefficients, latched on coef_tick), or what a note's start event-holds.
;; DGen schedules pure math over latches on their trigger, and a modulated
;; value mixed into it miscompiles (a data bell's G from its latched wref
;; and its mass went silent), so a param read with one counts as setup.
;;   f  a def form
(def slow-def? (f)
  (let ((e (nth f 2)))
    (or (member? slow-names (nth f 1))
        (head-of-3? e 'latch)
        (and (head-of-3? e 'event-hold) (= (nth e 2) 'hit)))))

;; The setup defs among forms: pure defs (no per-sample head, no per-sample
;; name, no defmacro of the instrument's own; slow-def? ones skipped)
;; reading one of params or another setup def. Returns the names they define.
;;   forms     the instrument's forms, in order
;;   params    the params' symbols
;;   stateful  library defmacros the forms call that keep state
(def setup-names (forms params stateful)
  (if (empty? params) (list)
    (let ((heads (append per-sample-heads stateful
                         (map (lambda (f) (nth f 1)) (filter (lambda (f) (head? f 'defmacro)) forms)))))
      (get (reduce (lambda (acc f)
                     (if (or (not (def-form? f)) (slow-def? f)) acc
                       (let ((atoms (form-atoms (nth f 2))) (dynamic (get acc :dynamic)) (setup (get acc :setup)))
                         (if (any? (lambda (a) (or (member? heads a) (member? dynamic a))) atoms)
                           (merge acc :dynamic (append dynamic (def-names f)))
                           (if (any? (lambda (a) (or (member? params a) (member? setup a))) atoms)
                             (merge acc :setup (append setup (def-names f)))
                             acc)))))
                   (dict :dynamic (list) :setup (list))
                   forms)
           :setup))))

;; Forms defining mod_tick, on which :modulate :all's held params are
;; sampled (see change-tick-forms): their knobs and the slow inputs
;; (slow-names) are watched every sample, the modulation sources every
;; mod-hold samples.
;;   params  the held params' symbols
(def mod-tick-forms (params) (change-tick-forms "mod" (append slow-names params)))

;; forms with specs' params modulatable (see above). A param no setup reads
;; is read as (mod p), by the work on an event clock (event-cones) as
;; (event-hold (mod p) clock); a clock's own change detection
;; (clock-form?) keeps reading the knob. With all, a param setup reads is
;; modulated too, held: setup defs read p_modh, other forms p_mod and the
;; setup values' latches, and the tick and each held param's p_modh / p_mod
;; follow the last param form. Each modulated param's form gets @mod.
;;   specs     the modulatable param specs
;;   all       true to modulate the params setup reads too
;;   forms     the instrument's forms, in order
;;   stateful  the library defmacros (use-defmacro) the forms call that keep
;;             state, so run every sample, as eseq.modal's voice-amp
(def mod-pass (specs all forms &key (stateful nil))
  (let ((params (map (lambda (p) (get p :sym)) specs))
        (param? (lambda (f) (head? f 'param)))
        (clock? (clock-form? forms))
        (cones (event-cones forms))
        (read-in (lambda (names fs) (filter (lambda (a) (member? names a)) (concat-all (map form-atoms fs))))))
    (let ((fed (read-in params (filter (defines? (setup-names forms params stateful)) forms)))
          (in-cone (lambda (f) (if (def-form? f) (cone-clock cones (first (def-names f))) nil))))
      (let ((direct (filter (lambda (p) (not (member? fed p))) params))
            (held (if all fed (list))))
        ;; the setup the held params feed, which runs on mod_tick
        (let ((hsetup (setup-names forms held stateful)))
          (let ((held-setup? (defines? hsetup))
                ;; how other code reads a modulated param, nil for any other atom
                (live (lambda (a) (if (member? direct a) (list 'mod a) (if (member? held a) (symbol a "_mod") nil)))))
            (let ((latched (read-in hsetup (filter (lambda (f) (not (or (held-setup? f) (param? f)))) forms)))
                  (last-param (reduce (lambda (k i) (if (param? (nth forms i)) i k)) 0 (range (len forms))))
                  (read-live (lambda (a) (let ((r (live a))) (if r r a)))))
              (concat-all
                (map (lambda (i)
                       (let ((f (nth forms i)) (clock (in-cone (nth forms i))))
                         (append
                           (if (param? f)
                             (list (if (live (nth f 1)) (append f '(@mod true @mod-mode additive)) f))
                             (if (clock? f) (list f)
                               (if clock (list (clock-held (rename-in f hsetup "_mod") live read-live clock))
                                 (if (held-setup? f)
                                   (cons (rename-in f held "_modh")
                                         (map (lambda (n) `(def ,(symbol n "_mod") (latch ,n mod_tick)))
                                              (filter (lambda (n) (member? latched n)) (def-names f))))
                                   (list (map-atoms (rename-in f hsetup "_mod") read-live))))))
                           (if (and (= i last-param) (not (empty? held)))
                             (append (mod-tick-forms held)
                                     (map (lambda (p) `(def ,(symbol p "_modh") (event-hold (mod ,p) mod_tick))) held)
                                     (concat-all (map held-value-forms held)))
                             (list)))))
                     (range (len forms)))))))))))

;; form with each modulated param read as clock holds it, (event-hold v
;; clock) with v how live reads it, for work DGen schedules on that clock: a
;; (mod p) mixed into it bare would run it every sample. Inside an
;; (event-hold x clock) already there a param is just v: holding twice
;; silences the instrument. (Under :modulate :all this work can also read
;; a setup value's mod_tick latch, the pattern held-value-forms avoids for
;; params: correct in every example, but not proven safe in general.)
;;   form       a form in clock's cone (see event-cones)
;;   live       a param's read form, or nil for any other atom
;;   read-live  an atom's read form: live's, or the atom
;;   clock      the clock's symbol
(def clock-held (form live read-live clock)
  (if (list? form)
    (if (head-of-3? form 'event-hold)
      (list 'event-hold (map-atoms (nth form 1) read-live) (nth form 2))
      (map (lambda (x) (clock-held x live read-live clock)) form))
    (let ((r (live form))) (if r `(event-hold ,r ,clock) form))))

;; The defs DGen computes on an event clock's schedule, as (name clock)
;; pairs in order: a def holding an (event-hold x clock) or reading one
;; already in a clock's cone, up to the (latch x clock) that brings it back
;; to audio rate.
;;   forms  the instrument's forms
(def event-cones (forms)
  (reduce (lambda (cones f)
            (if (or (not (def-form? f)) (head? (nth f 2) 'latch)) cones
              (let ((c (form-clock (nth f 2) cones)))
                (if c (append cones (map (lambda (n) (list n c)) (def-names f))) cones))))
          (list) forms))

;; The clock of the first event-hold in a form, or of the first name it
;; reads in one of cones; nil when none.
;;   form   a form
;;   cones  (name clock) pairs, from event-cones
(def form-clock (form cones)
  (if (list? form)
    (if (head-of-3? form 'event-hold) (nth form 2)
      (reduce (lambda (c x) (if c c (form-clock x cones))) nil form))
    (cone-clock cones form)))

;; The clock a name is in the cone of, or nil.
;;   cones  (name clock) pairs
;;   name   a symbol
(def cone-clock (cones name)
  (let ((hit (filter (lambda (pair) (= (first pair) name)) cones))) (if (empty? hit) nil (nth (first hit) 1))))

;; The forms defining a held param's p_mod: p_modh, taken on mod_tick and
;; kept in a history between ticks. A history, not a latch: DGen schedules a
;; latch on its trigger, so another clock's work reading it between
;; mod_ticks (a data bell's coefficients on release) gets garbage.
;;   p  the param's symbol
(def held-value-forms (p)
  (let ((h (symbol p "_hold")) (v (symbol p "_mod")))
    (list `(make-history ,h)
          `(def ,v (gswitch mod_tick ,(symbol p "_modh") (read-history ,h)))
          `(write-history ,h ,v))))

;; A predicate: true for a form of an event clock's change detection: the
;; def of one of forms' event-hold triggers (coef_tick, bynote_tick), or a
;; write to a history that def reads. Such a form must not read (mod p):
;; DGen gates each (mod p) on its routes being active, and a clock read in a
;; gated cone miscompiles what it clocks (the Ride Kit's wash went wrong),
;; so mod-pass leaves it reading the knobs.
;;   forms  the instrument's forms
(def clock-form? (forms)
  (let ((clocks (concat-all (map event-clocks forms))))
    (let ((clock-def? (defines? clocks)))
      (let ((watched (concat-all (map form-atoms (filter clock-def? forms)))))
        (lambda (f) (or (clock-def? f) (and (head-of-3? f 'write-history) (member? watched (nth f 1)))))))))

;; The triggers of every (event-hold x trigger) in a form.
;;   form  a form
(def event-clocks (form)
  (if (not (list? form)) (list)
    (append (if (head-of-3? form 'event-hold) (list (nth form 2)) (list))
            (concat-all (map event-clocks form)))))
