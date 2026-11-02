;; Latent Kick (DOOM + Neptunes) as an eseq.modal.train patch: the voice of
;; .local/research/latent-from-samples (voice/voice3.lisp) in the same decoder
;; layout, with its segment names (:names) so its rows and kick-rows' analysis load
;; as is.
;;
;;   eseq lisp run tools/modal/examples/train-kick.lisp   (from the repo root)
;;   tools/modal/train/run.py .local/train/kick-dn-v1
;;
;; The experiment: the research run's 33 MF DOOM kicks and 61 Neptunes / Pharrell kicks
;; (the library has more with those tags now; :only-kept holds the set), analysed from scratch (dgen analyze-drums + kick-rows),
;; refined, and trained into a 4-number latent: the research run's recipe.
(import eseq.modal.train :as t)

(def kick
  (t/voice "kick" :layout 'kick
    :parts (list
      ;; 12 modal slots, each with its own pitch drop (x drop_time, x bend), its own attack;
      ;; slot 0 a second, slower drop and a long tail
      (t/with-ranges (t/modes 'drum :count 12 :glide :per-mode :rise :per-mode :second-drop true :tail true :bus :body
                              :names '(f g td amp dec rise ph g2 t2 tamp tdec))
                     '(("f" 20 12000) ("dec" 0.001 6) ("tdec" 0.01 6)))
      ;; the beater: 4 short gliding sines
      (t/strikes 'stick :names '(xf xg xtd xamp xdec xph))
      ;; a clipped sine gliding down under the body, and a windowed mid burst
      (t/growl 'growl :names '(gr_amp gr_f gr_g gr_td gr_clip gr_d1 gr_d2 gr_m))
      (t/burst 'burst :names '(bu_amp bu_f bu_tc bu_w bu_ph))
      (t/shaper 'body :names '(sh_in sh_odd sh_even))
      (t/bark 'bark :names '(bk_amt))
      (t/clipper 'clip :names '(c_pos c_neg c_off c_mix))
      (t/record-lp 'rec :names '(rl_hz rl_mix))
      (t/with-ranges (t/timing 'time :rhythm false :names '(t0 end fade gain)) '(("end" 0.02 1.2)))
      ;; the record: a 16-band noise bed (x noise x record), crackle, two noise events
      (t/with-ranges (t/bands 'hiss :t0 false :snap false :level '(noise record) :decay-knob 'decay
                              :names '(nl nm nd1 nd2 nrise))
                     '(("wd1" 0.0002 0.5) ("wd2" 0.002 6)))
      (t/crackle 'crackle :names '(cr_lvl cr_rate cr_col cr_dec))
      (t/with-ranges (t/events 'ev :on 'hiss :rhythm false :names '(ev_t ev_lvl ev_dec ev_rise ev_col))
                     '(("ev_t" 0 0.8))))))

(def v1
  (t/experiment "kick-dn-v1" :voice kick
    :notes "the 94 MF DOOM + Neptunes kicks from scratch: the research run (runs/final_n94.sh) through eseq.modal.train"
    :data (t/data :tags '(("kick" "mf doom") ("kick" "neptunes"))
                  ;; the research run's files and names (the library's whole samples, uncropped)
                  :keep-crops ".local/research/latent-from-samples/data/samples94" :only-kept true
                  :crop :none)
    :seed (t/seed :analyze :kick)
    :train (t/recipe (t/refine :steps 250 :lr 0.02 :freeze :roles)
                     (t/refine :steps 150 :lr 0.005 :wave 0.3 :freeze '())
                     (t/pick :band 0.3 :mr 1)
                     (t/pretrain :steps 3000)
                     (t/path 0.05 :steps 4000))
    :ship (t/ship "Latent Kick (DOOM + Neptunes) v2" :member "kick" :tone '(tune decay drive level)
                  :extra-knobs '(growl punch))))

(t/emit-experiment v1 ".local/train/kick-dn-v1")
