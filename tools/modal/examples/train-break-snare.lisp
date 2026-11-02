;; Latent Break Snare v2 (.local/research/break-snares voice/snare3.lisp) as an
;; eseq.modal.train patch: the same parts in the same decoder layout, with the
;; hand-written voice's segment names (:names) so its trained rows load as is.
;;
;;   eseq lisp run tools/modal/examples/train-break-snare.lisp   (from the repo root)
;;   tools/modal/train/run.py .local/train/break-snare-v3
;;
;; The experiment below is Latent Break Snare v3: pack `set` (breaks) + the 32
;; MF DOOM snares of the original Latent Snare (Samples), starting from v2's
;; fitted rows (the v2 run) for the samples v2 knew.
(import eseq.modal.train :as t)

;; PM Ride, character Classic: the 32 ring modes (>= 30 ms) with the most
;; energy (break-snares voice/ride_template.json): Hz, decay s, unit-norm
;; magnitude, phase.
(def ride-freqs '(261.756 323.464 462.128 465.5 523.75 523.912 526.819 531.072 1530.74 1708.97 1710.57 1711.08 2597.32 2697.08 2755.78 2790.45 3586.04 3588.04 3596.92 3643.53 3643.89 3776.48 3783.99 3795.82 4051.22 4909.01 6361.45 6550.97 6647.86 7737.49 9551.14 524.675))
(def ride-taus '(1.37193 1.77661 0.43916 0.76142 0.61912 0.4397 0.34016 0.32542 0.46168 0.2907 0.22718 0.19487 1.05317 0.35795 0.8111 0.98325 0.21971 0.13626 0.2437 0.36678 0.29614 0.56681 0.92852 0.99981 0.10314 0.17174 0.14817 0.16817 0.31808 0.1497 0.07538 0.03102))
(def ride-mags '(0.06915 0.054361 0.118815 0.068462 0.126516 0.176814 0.111313 0.117438 0.082934 0.111278 0.137409 0.180081 0.088635 0.140974 0.092595 0.064358 0.123529 0.334302 0.290684 0.238308 0.22433 0.106623 0.066723 0.163384 0.25714 0.288078 0.193026 0.156197 0.0994 0.159984 0.243598 0.349664))
(def ride-phases '(0.786 2.49577 1.73218 -1.72657 -1.25559 2.34711 -3.10851 2.01834 1.86654 -0.20147 -1.23758 -1.39219 -1.5402 -0.3451 0.02858 0.33613 3.11332 1.83885 0.76767 3.07223 -1.78877 -2.13495 0.70711 -2.8655 -2.91741 0.09355 -0.21233 2.62114 0.81195 0.0887 -0.01964 -1.58641))

(def break-snare
  (t/voice "break-snare" :layout 'snare
    :parts (list
      ;; the drum head: 16 note-tracked modes on one shared tension glide, a slow tail on slot 0
      (t/modes 'head :count 16 :glide true :tail true :rise :shared :bus :body
               :names '(f amp dec rise ph g td tamp tdec))
      ;; the stick: 4 short gliding sines
      (t/strikes 'stick :names '(xf xg xtd xamp xdec xph))
      (t/shaper 'body :names '(sh_in sh_odd sh_even))
      (t/clipper 'clip :names '(c_pos c_neg c_off c_mix))
      (t/record-lp 'rec :names '(rl_hz rl_mix))
      (t/timing 'time :names '(t0 end fade gain))
      ;; snare wires: 16 noise bands, fast + slow decay
      (t/bands 'wires :names '(wl wm wd1 wd2 w_t0 wrise))
      ;; record: hiss on the wires' bands, crackle, two noise events
      (t/hiss 'hiss :on 'wires :names '(hl hdec))
      (t/crackle 'crackle :names '(cr_lvl cr_rate cr_col cr_dec))
      (t/events 'ev :on 'wires :names '(ev_t ev_lvl ev_dec ev_rise ev_col))
      ;; metal: 8 free partials 1-18 kHz (hats, tambourine, a ride already ringing)
      (t/modes 'metal :count 8 :follow-note false :rise :instant :strokes true :bus :drum :knob 'metal
               :ranges (dict :f '(1000 18000) :amp '(0.0000001 2) :dec '(0.005 6))
               :names '(mf ma md mph))
      ;; the ride: a measured ride's ring modes, warped; its wash on the wires' bands
      (t/template 'ride :freqs ride-freqs :taus ride-taus :mags ride-mags :phases ride-phases
                  :knob 'metal :strokes true :names '(rb_size rb_lvl rb_tilt rb_dec))
      (t/wash 'wash :on 'wires :knob 'metal :strokes true :names '(rw_l rw_rise rw_dec))
      ;; a second stroke a 16th or so later (ride, flam)
      (t/restrike 'second :names '(mr_t mr_lvl)))))

(def v3
  (t/experiment "break-snare-v3" :voice break-snare
    :notes "v2's voice on pack set + the MF DOOM snares (eseq-w9yz.6)"
    :data (t/data :packs '("set") :tags '(("snare" "mf doom"))
                  ;; the 37 breaks v1 / v2 trained on keep their exact crops
                  :keep-crops ".local/research/break-snares/data/train")
    :seed (t/seed :parent ".local/research/break-snares/runs/m2_rA" :analyze :drum
                  :parent-voice ".local/research/break-snares/voice/snare3.lisp"
                  :level '((metal 0.3) (ride 0.1) (wash 0.03)))
    :train (t/recipe (t/refine :steps 400 :lr 0.02 :freeze :roles :refit :new)
                     (t/refine :steps 200 :lr 0.005 :wave 0.3 :freeze '() :refit :new)
                     (t/pick :band 0.3 :mr 1)
                     (t/pretrain :steps 3000)
                     (t/path 0.02 :steps 4000))
    :ship (t/ship "Latent Break Snare v3" :folder "Research")))

(t/emit-experiment v3 ".local/train/break-snare-v3")

;; v4: a variant of v3's voice for the ear test of v2 (eseq-w9yz.5):
;;   - B28's ride is "missing its rhythm": its first-stroke ride trained to ~0, so a second stroke
;;     relative to it is ~0 too. The ride body and wash get their own second-stroke levels.
;;   - "not ultra heavy": the ride template keeps its 16 strongest modes (of 32), about half its CPU.
;;   - a 6-number latent (v3-l6: 85 snares need the room).
;; It starts from v3's fitted rows: the absolute levels are seeded as v3's relative ones (same sound),
;; then every sample is refit.
(def break-snare-v4
  (t/variant break-snare "break-snare-v4" :latent 6
    :swap (list (t/template 'ride :freqs ride-freqs :taus ride-taus :mags ride-mags :phases ride-phases
                            :keep 16 :knob 'metal :strokes true :second :absolute
                            :names '(rb_size rb_lvl rb_tilt rb_dec rb_lvl2))
                (t/wash 'wash :on 'wires :knob 'metal :strokes true :second :absolute
                        :names '(rw_l rw_rise rw_dec rw_l2)))))

(def v4
  (t/experiment "break-snare-v4" :voice break-snare-v4
    :notes "v3 + absolute second-stroke ride levels (B28) + a 16-mode ride (CPU)"
    :data (t/data :packs '("set") :tags '(("snare" "mf doom")) :keep-crops ".local/train/break-snare-v3/data")
    :seed (t/seed :parent ".local/train/break-snare-v3" :analyze :drum)
    :train (t/recipe (t/refine :steps 300 :lr 0.01 :freeze :roles :refit :all)
                     (t/pick :band 0.3 :mr 1)
                     (t/pretrain :steps 3000)
                     (t/path 0.02 :steps 4000))
    :ship (t/ship "Latent Break Snare v4" :folder "Research")))

(t/emit-experiment v4 ".local/train/break-snare-v4")

;; v3 with a 6-number latent: 85 varied snares morph much more smoothly with room for them (a training
;; test on v3's fitted rows: straight-blend CV 0.34 vs 0.49, band 0.83 vs 0.92). Same parts, same fitted
;; rows (the parent's); only the latent stages run.
(def v3l6
  (t/experiment "break-snare-v3-l6" :voice (t/variant break-snare "break-snare-l6" :latent 6)
    :notes "v3's voice and fitted rows with a 6-number latent"
    :data (t/data :packs '("set") :tags '(("snare" "mf doom")) :keep-crops ".local/train/break-snare-v3/data")
    :seed (t/seed :parent ".local/train/break-snare-v3")
    :train (t/recipe (t/pretrain :steps 3000) (t/path 0.02 :steps 4000))
    :ship (t/ship "Latent Break Snare v3 L6" :folder "Research")))

(t/emit-experiment v3l6 ".local/train/break-snare-v3-l6")
