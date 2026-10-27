;; A semi-hollow electric guitar (ES-335 style), built from finite-element
;; solids joined at named points, through a clean amp.
;;
;;   body   laminated maple, 44.5 mm deep, 490 mm long, 406 mm across the
;;          lower bout: a 3 mm top and back over two cavities, one each
;;          side of a solid maple centre block that runs from the tail
;;          block to the neck block, with f-holes cut into the top over
;;          them. Rims are 7 mm (one mesh cell; a real rim is ~2 mm plus
;;          its kerfing)
;;   neck   mahogany: a rounded back, a fretboard on top and a flat
;;          headstock, its own beam modes (the neck's dead spots), glued to
;;          the body's neck block at its heel
;;   string steel, 628 mm scale, its ends mounted on the centre block (the
;;          bridge) and the neck (the nut): their motion moves the ends.
;;          One way for now: the string's reaction on them (m/mount :back)
;;          is not yet energy-conserving and runs away on high notes
;;          (eseq-lojj.22), so the body and neck do not ring here.
;;          Its bending stiffness follows the note: a wound string's (its
;;          core's) below G3, a plain .010-.017's above
;;   pick   a pick dragged through the string until it slips off
;;   pickups two humbuckers (neck and bridge) hearing the string
;;   mic    a little of the hollow body's own sound, by the bass f-hole
;;   amp    clean: little drive, no bias or bloom, a soft power stage
;;
;; Masses are relative to the string's (1): a guitar's string vibrates
;; about a tenth of a gram against a few hundred grams of body at the
;; bridge, so the body is 300 and the neck 100.
;;
;; Both solids are wood with a :grain, so they are orthotropic: stiff along
;; the grain, an order of magnitude softer across it. They damp as wood
;; does, each mode losing pi * eta * f per second (:damping, eta): a body
;; mode at 250 Hz rings about 0.1 s, one at 2 kHz a hundredth of that.
;; Maple alone is eta 0.012; the built body's 0.03 adds its glue joints and
;; the sound its top radiates (measured guitar top modes have Q 20-60), the
;; neck's 0.02 its fretboard and set joint. Their cells are flat
;; (:cell-size, x y z): 1.5 mm through the body's depth puts two layers in
;; its 3 mm top, at 6 mm across it. Solving the body takes a few minutes
;; the first time (then it is cached).
;;
;; x runs along the guitar toward the headstock, y across it, z up out of
;; the top (the top at z = 22.25 mm). The neck is seated on the body by the
;; 'joint spring (its :heel on the body's :neck-joint); open the script in
;; the Shape Lab and pick Instrument to see the whole guitar, and turn a
;; connection's "point" knob to try the other named spots.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-hollow-body" :voicing :poly
  :objects (list (m/finger 'pick)
                 ;; :radius-mm sets only the bending stiffness (the partials'
                 ;; stretch): a wound string's is its thin core's over its
                 ;; whole weight, about a 0.12 mm plain string's
                 (m/string 'str :made-of mat/steel :length 0.628 :modes 60
                           :radius-mm (m/by-note 82 0.12 160 0.13 196 0.2 247 0.165 330 0.127 1320 0.127)
                           :const-loss 0.15 :freq-loss 0.08 :mute 30)
                 (m/solid 'body :made-of mat/maple :grain (list :x :y) :modes 48 :loss 3 :hf-loss 0 :damping 0.03 :mass 300
                          :resolution (dict :cell-size (list 0.006 0.006 0.0015))
                          :shape (dict :difference (list (dict :union (list (dict :cylinder (dict :axis :z :center (list -0.06 0 0) :height 0.0445 :radius 0.203))
                                                                            (dict :cylinder (dict :axis :z :center (list 0.08 0 0) :height 0.0445 :radius 0.15))))
                                                         ;; the waist
                                                         (dict :cylinder (dict :axis :z :center (list 0.03 0.255 0) :height 0.06 :radius 0.1))
                                                         (dict :cylinder (dict :axis :z :center (list 0.03 -0.255 0) :height 0.06 :radius 0.1))
                                                         ;; the cutaways, either side of the neck block
                                                         (dict :cylinder (dict :axis :z :center (list 0.215 0.075 0) :height 0.06 :radius 0.045))
                                                         (dict :cylinder (dict :axis :z :center (list 0.215 -0.075 0) :height 0.06 :radius 0.045))
                                                         ;; the bass-side cavity: the outline 7 mm in and
                                                         ;; 3 mm under the top and over the back, short
                                                         ;; of the centre block (|y| < 25 mm) and of the
                                                         ;; tail and neck blocks
                                                         (dict :difference (list (dict :union (list (dict :cylinder (dict :axis :z :center (list -0.06 0 0) :height 0.0385 :radius 0.196))
                                                                                                    (dict :cylinder (dict :axis :z :center (list 0.08 0 0) :height 0.0385 :radius 0.143))))
                                                                                 (dict :box (dict :center (list 0 -0.225 0) :size (list 1 0.5 0.1)))
                                                                                 (dict :box (dict :center (list 0.44 0 0) :size (list 0.5 1 0.1)))
                                                                                 (dict :box (dict :center (list -0.5 0 0) :size (list 0.5 1 0.1)))
                                                                                 (dict :cylinder (dict :axis :z :center (list 0.03 0.255 0) :height 0.06 :radius 0.107))
                                                                                 (dict :cylinder (dict :axis :z :center (list 0.215 0.075 0) :height 0.06 :radius 0.052))))
                                                         ;; the treble-side cavity, its mirror image
                                                         (dict :difference (list (dict :union (list (dict :cylinder (dict :axis :z :center (list -0.06 0 0) :height 0.0385 :radius 0.196))
                                                                                                    (dict :cylinder (dict :axis :z :center (list 0.08 0 0) :height 0.0385 :radius 0.143))))
                                                                                 (dict :box (dict :center (list 0 0.225 0) :size (list 1 0.5 0.1)))
                                                                                 (dict :box (dict :center (list 0.44 0 0) :size (list 0.5 1 0.1)))
                                                                                 (dict :box (dict :center (list -0.5 0 0) :size (list 0.5 1 0.1)))
                                                                                 (dict :cylinder (dict :axis :z :center (list 0.03 -0.255 0) :height 0.06 :radius 0.107))
                                                                                 (dict :cylinder (dict :axis :z :center (list 0.215 -0.075 0) :height 0.06 :radius 0.052))))
                                                         ;; the f-holes, through the top into the cavities
                                                         (dict :box (dict :center (list -0.04 0.11 0.0207) :size (list 0.12 0.012 0.008)))
                                                         (dict :box (dict :center (list -0.04 -0.11 0.0207) :size (list 0.12 0.012 0.008)))))
                          :points (dict :back (list -0.06 0.1 -0.02225)
                                        :bass-f-hole (list -0.04 0.13 0.02225)
                                        :bridge (list 0.02 0 0.02225)
                                        :neck-joint (list 0.228 0 0.012)
                                        :tailpiece (list -0.09 0 0.02225)
                                        :treble-wing (list -0.08 -0.13 0.02225)
                                        :upper-horn (list 0.14 0.11 0.02225)))
                 (m/solid 'neck :made-of mat/mahogany :grain (list :x :z) :modes 24 :loss 3 :hf-loss 0 :damping 0.02 :mass 100
                          :resolution (dict :cell-size (list 0.006 0.004 0.002))
                          :shape (dict :union (list ;; the shaft: a rounded back under a flat top
                                                    (dict :difference (list (dict :cylinder (dict :axis :x :center (list 0.21 0 0.0085) :height 0.42 :radius 0.0245))
                                                                            (dict :box (dict :center (list 0.21 0 0.05) :size (list 0.5 0.1 0.1)))))
                                                    ;; the fretboard
                                                    (dict :box (dict :center (list 0.21 0 0.003) :size (list 0.42 0.048 0.006)))
                                                    ;; the headstock
                                                    (dict :box (dict :center (list 0.51 0 -0.004) :size (list 0.18 0.09 0.015)))))
                          :points (dict :fret-12 (list 0.106 0 0.006)
                                        :headstock (list 0.55 0 0.0035)
                                        :heel (list 0 0 -0.005)
                                        :nut (list 0.42 0 0.006))))
  :connections (list (m/pluck 'plk 'pick 'str 0.86)
                     ;; the string's ends: the bridge (end 1) on the centre
                     ;; block, the nut (end 0) on the neck. A mount moves the
                     ;; end with its body; a spring near the end would
                     ;; stiffen the string and detune its partials (a bell)
                     (m/mount 'bridge 'body :bridge 'str 1)
                     (m/mount 'nut 'neck :nut 'str 0)
                     ;; the set neck, glued into the neck block
                     (m/spring 'joint 'neck :heel 'body :neck-joint :stiffness 40))
  :listens (list (m/pickup 'str 0.92 :height 3 :coil-hz 2600 :coil-q 1.6)
                 (m/pickup 'str 0.75 :height 3.5 :coil-hz 2300 :coil-q 1.4 :gain 0.8)
                 (m/listen 'body :bass-f-hole :gain 0.5))
  :amp (m/amp :drive 2 :bias 0 :bloom 0 :bass 1.05 :mid 0.9 :treble 1.1 :power 0.8 :sag 0.05 :presence 0.6
              :cab 5500 :level 0.7))
