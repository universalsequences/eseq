;; eseq.modal — Modalys-style instruments from objects and connections.
;;
;; A patch is plain data: objects (string, plate, membrane, finger, mallet),
;; connections between their access points (pluck, spring, felt, bow), and
;; listening points. (dsp patch) turns it into the source of an ordinary
;; DGenLisp instrument calling the protocol macros in content/defmacros
;; (modal-predict, modal-point, a force law, modal-advance), in the order one
;; sample needs:
;;   1 predict every object   2 + 3 solve the exciters (pluck, felt) in turn,
;;   each reading the objects as earlier ones left them, then springs and
;;   bows by Gauss-Seidel sweeps   4 commit every object
;; Every name an object or connection introduces is prefixed with its id, so
;; two strings never collide; their params are prefixed the same way and
;; grouped by id, so the instrument panel shows one section per object.
;;
;;   (import eseq.modal :as m)
;;   (m/patch :name "my-cello"
;;     :objects     (list (m/string 'str :material :nylon) (m/plate 'body))
;;     :connections (list (m/bow 'bow 'str 0.12)
;;                        (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
;;     :listens     (list (m/listen 'str 0.85)))
;;
;; (check patch) lists what is wrong with a patch by name ("spring bridge: no
;; object bdy"); (dsp patch) assumes a patch that checks clean.

(module eseq.modal)

(export string plate membrane finger mallet
        pluck spring felt bow listen patch
        check dsp ui generate materials)

;; ── small helpers ───────────────────────────────────────────────────────────
(def nm (id suffix) (symbol id "_" suffix))
(def concat-all (lists) (apply append lists))
(def sum-form (terms)
  (if (empty? terms) 0 (if (= (len terms) 1) (first terms) (cons '+ terms))))

;; in the order of content/defmacros/modal-material's table
(def material-keys ()
  (list :uranium :steel :brass :aluminium :granite :nylon :carbon :diamond :spruce))
(def materials () (map bare (material-keys)))

;; A material as given (a keyword from materials, or its index) -> its index,
;; or nil when it names no material.
(def material-index (m)
  (if (number? m) m
    (let ((hits (filter (lambda (k) (= (nth (material-keys) k) m)) (range (len (material-keys))))))
      (if (empty? hits) nil (first hits)))))

;; A param spec: its DGenLisp symbol, group and range; options makes a menu.
;; title, taper (:linear / :log) and dec(imals) are for the generated panel.
(def pspec (id name default lo hi &key (options nil) (unit nil) (title nil) (taper :linear) (dec nil))
  (dict :sym (nm id name) :group id :default default :min lo :max hi :options options :unit unit
        :title (if title title name) :taper taper
        :dec (if dec dec (if (<= hi 10) 2 (if (<= hi 100) 1 0)))))
(def param-form (p)
  (if (get p :options)
    `(param ,(get p :sym) @group ,(get p :group) @options [ ,@(get p :options) ] @default ,(get p :default))
    (append `(param ,(get p :sym) @group ,(get p :group) @default ,(get p :default) @min ,(get p :min) @max ,(get p :max))
            (if (get p :unit) `(@unit ,(get p :unit)) nil))))

(def material-param (id material)
  (pspec id "material" (material-index material) 0 (- (len (material-keys)) 1)
         :options (materials) :title "Material"))

;; ── objects ─────────────────────────────────────────────────────────────────
;; :kind picks the emitters below; :lumped objects have one coordinate (scalar
;; state, phi = 1) instead of a mode vector. :given keeps what the patch
;; author wrote, for check.
(def string (id &key (material :steel) (length 0.65) (radius-mm 0.5) (modes 60)
                 (freq-loss 0.4) (const-loss 0.3) (mute 6) (tune 1))
  (dict :id id :title "STRING" :kind :string :lumped false :modes modes :stuff material
        :params (list (material-param id material)
                      (pspec id "length" length 0.2 20 :unit 'm :title "Length" :taper :log)
                      (pspec id "radius" radius-mm 0.1 5 :unit 'mm :title "Radius" :taper :log)
                      (pspec id "hf_loss" freq-loss 0 10 :title "HF loss")
                      (pspec id "loss" const-loss 0 20 :title "Loss")
                      (pspec id "mute" mute 0 60 :title "Mute")
                      (pspec id "tune" tune 0.25 4 :title "Tune" :dec 3))))

(def plate (id &key (material :spruce) (thick-mm 3) (width 0.45) (height 0.35)
                (mass 30) (loss 4) (damp 0.3))
  (dict :id id :title "PLATE" :kind :plate :lumped false :modes 64 :stuff material
        :params (list (material-param id material)
                      (pspec id "thick" thick-mm 0.5 20 :unit 'mm :title "Thick" :taper :log)
                      (pspec id "mass" mass 1 1000 :title "Mass" :taper :log)
                      (pspec id "width" width 0.05 2 :title "Width")
                      (pspec id "height" height 0.05 2 :title "Height")
                      (pspec id "loss" loss 0 40 :title "Loss")
                      (pspec id "damp" damp 0 5 :title "HF damp"))))

(def membrane (id &key (loss 6) (bright 1) (choke 0) (modes 120) (tune 1))
  (dict :id id :title "HEAD" :kind :membrane :lumped false :modes 120
        :params (list (pspec id "loss" loss 0 60 :title "Decay")
                      (pspec id "bright" bright 0 10 :title "Dark")
                      (pspec id "choke" choke 0 60 :title "Choke")
                      (pspec id "tune" tune 0.25 4 :title "Tune" :dec 3)
                      (pspec id "modes" modes 1 120 :title "Modes" :dec 0))))

;; a finger (Modalys' two-mass): a hand on a trajectory (from +start to
;; -depth over stroke_ms) with a soft tip hung off it
(def finger (id &key (hz 300) (damp 0.5) (stiff 2) (start 0.1) (depth 0.2) (stroke-ms 30))
  (dict :id id :title "FINGER" :kind :two-mass :lumped true
        :params (list (pspec id "hz" hz 20 2000 :unit 'Hz :title "Flesh" :taper :log)
                      (pspec id "damp" damp 0.05 0.9 :title "Damp")
                      (pspec id "stiff" stiff 0.1 20 :title "Stiff" :taper :log)
                      (pspec id "start" start 0.01 0.5 :title "From")
                      (pspec id "depth" depth 0.01 1 :title "To")
                      (pspec id "stroke" stroke-ms 1 1000 :unit 'ms :title "Time" :taper :log))))

;; a thrown mass; mass is relative to one modal mass of what it strikes
(def mallet (id &key (mass 0.01) (speed 1))
  (dict :id id :title "MALLET" :kind :mallet :lumped true
        :params (list (pspec id "mass" mass 0.002 0.5 :title "Mass" :taper :log :dec 3)
                      (pspec id "speed" speed 0.1 4 :title "Speed"))))

;; ── connections ─────────────────────────────────────────────────────────────
;; a is the driver/upper end, b the driven one: the law's F pushes b by +F
;; and a by -F. Access positions become params (strings: 0..1; plates: x y;
;; membranes: r theta).
(def pluck (id a b at &key (stiff 20) (threshold 0.05))
  (dict :id id :title "PLUCK" :law :pluck :a a :b b :a-at nil :b-at at
        :params (list (pspec id "stiff" stiff 0.5 200 :title "Grip" :taper :log :dec 1)
                      (pspec id "thresh" threshold 0.002 0.3 :title "Release" :taper :log :dec 3))))

(def spring (id a a-at b b-at &key (stiffness 2))
  (dict :id id :title "SPRING" :law :spring :a a :b b :a-at a-at :b-at b-at
        :params (list (pspec id "stiff" stiffness 0 50 :title "Glue" :dec 1))))

(def felt (id a b b-at &key (felt 0.005) (hardness 0.5))
  (dict :id id :title "FELT" :law :felt :a a :b b :a-at nil :b-at b-at
        :params (list (pspec id "felt" felt 0.0005 5 :title "Felt" :taper :log :dec 4)
                      (pspec id "hardness" hardness 0 1 :title "Hard"))))

;; a bow drives b's access point at a steady speed while the note is held;
;; it has no object of its own (Modalys drives the bow through a position
;; connection; here its speed follows the gate through attack / release)
(def bow (id b at &key (pressure 0.2) (speed 0.2) (slip 1) (attack-ms 60) (release-ms 120))
  (dict :id id :title "BOW" :law :bow :a nil :b b :a-at nil :b-at at
        :params (list (pspec id "pressure" pressure 0.01 2 :title "Pressure" :taper :log)
                      (pspec id "speed" speed 0.01 1 :title "Speed" :taper :log)
                      (pspec id "slip" slip 0.1 3 :title "Slip")
                      (pspec id "attack" attack-ms 1 1000 :unit 'ms :title "Attack" :taper :log)
                      (pspec id "release" release-ms 1 2000 :unit 'ms :title "Release" :taper :log))))

(def listen (obj at &key (gain 1)) (dict :obj obj :at at :gain gain))

;; :name is what Save offers as the instrument's name
(def patch (&key (name "modal") (objects nil) (connections nil) (listens nil) (level 1))
  (dict :name name :objects objects :connections connections :listens listens :level level))

;; ── check ───────────────────────────────────────────────────────────────────
(def find-object (patch id)
  (first (filter (lambda (o) (= (get o :id) id)) (get patch :objects))))

(def bare (k) (let ((s (source k))) (substring s 1 (len s))))
(def law-name (c) (bare (get c :law)))
(def what (c) (str (law-name c) " " (source (get c :id))))

;; What is wrong with access point at on obj, as a list of messages.
(def at-errors (who obj at)
  (if (get obj :lumped) nil
    (if (= (get obj :kind) :string)
      (if (number? at) nil
        (list (str who ": string " (source (get obj :id)) " takes one position 0..1, got " (source at))))
      (if (number? at)
        (list (str who ": " (bare (get obj :kind)) " " (source (get obj :id))
                   " takes a position (list x y), got " (source at)))
        nil))))

(def end-errors (patch c side id at)
  (let ((obj (find-object patch id)))
    (if (= obj nil)
      (list (str (what c) ": no object " (source id)))
      (at-errors (str (what c) " " side) obj at))))

(def connection-errors (patch c)
  (append (if (= (get c :a) (get c :b)) (list (str (what c) ": both ends on " (source (get c :b)))) nil)
          (if (get c :a) (end-errors patch c "a" (get c :a) (get c :a-at)) nil)
          (end-errors patch c "b" (get c :b) (get c :b-at))))

(def listen-errors (patch l)
  (let ((obj (find-object patch (get l :obj))))
    (if (= obj nil)
      (list (str "listen: no object " (source (get l :obj))))
      (at-errors "listen" obj (get l :at)))))

(def duplicate-ids (patch)
  (let ((ids (append (map (lambda (o) (get o :id)) (get patch :objects))
                     (map (lambda (c) (get c :id)) (get patch :connections)))))
    (map (lambda (k) (str "two things are called " (source (nth ids k))))
         (filter (lambda (k) (not (empty? (filter (lambda (j) (= (nth ids j) (nth ids k))) (range k)))))
                 (range (len ids))))))

(def material-errors (patch)
  (concat-all
    ;; objects without a material (membrane, finger, mallet) have no :stuff
    (map (lambda (o) (if (and (not (= (get o :stuff) nil)) (= (material-index (get o :stuff)) nil))
                       (list (str (bare (get o :kind)) " " (source (get o :id)) ": no material "
                                  (source (get o :stuff)) "; one of " (source (material-keys))))
                       nil))
         (get patch :objects))))

;; Everything wrong with patch, as a list of messages; empty when it is fine.
(def check (patch)
  (concat-all
    (list (if (empty? (get patch :objects)) (list "a patch needs at least one object") nil)
          (if (empty? (get patch :listens)) (list "a patch needs at least one listen point") nil)
          (duplicate-ids patch)
          (material-errors patch)
          (concat-all (map (lambda (c) (connection-errors patch c)) (get patch :connections)))
          (concat-all (map (lambda (l) (listen-errors patch l)) (get patch :listens))))))

;; ── access-point params ─────────────────────────────────────────────────────
;; One coordinate per param: strings take one, plates and membranes two.
;; label titles the coordinates (default: the object's id)
(def access-params (cid side obj at &key (label nil))
  (let ((on (source (get obj :id))) (lb (if label label (source (get obj :id)))))
    (if (get obj :lumped) nil
      (if (= (get obj :kind) :string)
        (list (pspec cid (str on "_at") at 0.01 0.99 :title (str lb " at")))
        (if (= (get obj :kind) :plate)
          (list (pspec cid (str on "_x") (nth at 0) 0.01 0.99 :title (str lb " x"))
                (pspec cid (str on "_y") (nth at 1) 0.01 0.99 :title (str lb " y")))
          (list (pspec cid (str on "_r") (nth at 0) 0 0.95 :title (str lb " r"))
                (pspec cid (str on "_angle") (nth at 1) 0 3.14159 :title (str lb " angle"))))))))

(def access-syms (cid side obj)
  (map (lambda (p) (get p :sym)) (access-params cid side obj (list 0 0))))

;; Forms defining phi-sym, an object's mode-shape vector at access coordinates.
(def shape-defs (obj coords phi-sym)
  (let ((id (get obj :id)) (kind (get obj :kind)))
    (if (= kind :string)
      (list `(def ,phi-sym (* (sin (* ,(nm id "n") 3.14159265 ,(first coords))) ,(nm id "mask"))))
      (if (= kind :plate)
        (list `(def ,phi-sym (* (rect-plate-shape ,(nm id "pm") ,(nm id "pn") ,(nth coords 0) ,(nth coords 1))
                                ,(nm id "mask"))))
        (list `(def (,(nm phi-sym "ratios") ,(nm phi-sym "raw")) (circ-membrane ,(nth coords 0) ,(nth coords 1)))
              `(def ,phi-sym (* ,(nm phi-sym "raw") ,(nm id "mask"))))))))

;; ── per-object DSP ──────────────────────────────────────────────────────────
(def p (obj name) (nm (get obj :id) name))

(def object-setup (obj)
  (let ((id (get obj :id)) (kind (get obj :kind)) (m (get obj :modes)))
    (if (= kind :string)
      (list `(def (,(p obj "density") ,(p obj "young")) (modal-material ,(p obj "material")))
            `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
            `(def ,(p obj "n") (+ (iota ,m) 1))
            `(def ,(p obj "freqs") (stiff-string-freqs ,(p obj "n") ,(p obj "f0") ,(p obj "young") ,(p obj "density")
                                     (* ,(p obj "radius") 0.001) ,(p obj "length")))
            `(def ,(p obj "mask") (modal-band ,(p obj "n") ,(p obj "freqs") ,m))
            `(def ,(p obj "sigma") (+ (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "hf_loss"))
                                     (* ,(p obj "mute") (- 1 gate))))
            `(def (,(p obj "a") ,(p obj "b")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))
            `(def ,(p obj "wref") (min (* twopi (/ ,(p obj "f0") samplerate)) 2.8))
            `(def ,(p obj "mass") 1)
            `(def ,(p obj "G") (* ,(p obj "wref") ,(p obj "wref")))
            `(make-history ,(p obj "s1") @shape [ ,m ])
            `(make-history ,(p obj "s2") @shape [ ,m ]))
    (if (= kind :plate)
      (list `(def ,(p obj "grid") (iota 64))
            `(def ,(p obj "pm") (+ (floor (/ ,(p obj "grid") 8)) 1))
            `(def ,(p obj "pn") (+ (- ,(p obj "grid") (* 8 (floor (/ ,(p obj "grid") 8)))) 1))
            `(def (,(p obj "density") ,(p obj "young")) (modal-material ,(p obj "material")))
            `(def ,(p obj "thick_m") (* ,(p obj "thick") 0.001))
            `(def ,(p obj "freqs") (rect-plate ,(p obj "pm") ,(p obj "pn") ,(p obj "young") ,(p obj "density")
                                     ,(p obj "thick_m") ,(p obj "width") ,(p obj "height")))
            `(def ,(p obj "mask") (modal-band (+ ,(p obj "grid") 1) ,(p obj "freqs") 64))
            `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "damp")))
            `(def (,(p obj "a") ,(p obj "b")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))
            `(def ,(p obj "f11") (rect-plate 1 1 ,(p obj "young") ,(p obj "density")
                                   ,(p obj "thick_m") ,(p obj "width") ,(p obj "height")))
            `(def ,(p obj "wref") (min (* twopi (/ ,(p obj "f11") samplerate)) 2.8))
            `(def ,(p obj "G") (/ (* ,(p obj "wref") ,(p obj "wref")) ,(p obj "mass")))
            `(make-history ,(p obj "s1") @shape [ 64 ])
            `(make-history ,(p obj "s2") @shape [ 64 ]))
    (if (= kind :membrane)
      (list `(def (,(p obj "ratios") ,(p obj "phi0")) (circ-membrane 0 0))
            `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
            `(def ,(p obj "freqs") (* ,(p obj "ratios") ,(p obj "f0")))
            `(def ,(p obj "mask") (modal-band (+ (iota 120) 1) ,(p obj "freqs") ,(p obj "modes")))
            `(def ,(p obj "sigma") (+ (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "bright")) ,(p obj "choke")))
            `(def (,(p obj "a") ,(p obj "b")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))
            `(def ,(p obj "wref") (min (* twopi (/ ,(p obj "f0") samplerate)) 2.8))
            `(def ,(p obj "mass") 1)
            `(def ,(p obj "G") (* ,(p obj "wref") ,(p obj "wref")))
            `(make-history ,(p obj "s1") @shape [ 120 ])
            `(make-history ,(p obj "s2") @shape [ 120 ]))
    (if (= kind :two-mass)
      (list `(def ,(p obj "frac") (min (/ (* age 1000) ,(p obj "stroke")) 1))
            `(def ,(p obj "hand") (- ,(p obj "start") (* (+ ,(p obj "start") ,(p obj "depth")) ,(p obj "frac"))))
            `(def (,(p obj "a") ,(p obj "b") ,(p obj "follow") ,(p obj "C"))
               (two-mass-coefs ,(p obj "hz") ,(p obj "damp") ,(p obj "stiff")))
            `(make-history ,(p obj "s1"))
            `(make-history ,(p obj "s2")))
    ;; mallet
      (list `(def ,(p obj "drop") 0.001)
            `(def ,(p obj "throw") (/ (* ,(p obj "speed") (+ 0.2 (* 0.8 hit_velocity))) samplerate))
            `(def ,(p obj "wref") (min (* twopi (/ f0 samplerate)) 2.8))
            `(def ,(p obj "C") (/ (* ,(p obj "wref") ,(p obj "wref")) ,(p obj "mass")))
            `(make-history ,(p obj "hx"))
            `(make-history ,(p obj "hv")))))))))

(def object-predict (obj)
  (let ((kind (get obj :kind))
        (free `(modal-predict (read-history ,(p obj "s1")) (read-history ,(p obj "s2")) ,(p obj "a") ,(p obj "b"))))
    (if (= kind :two-mass)
      (list `(def ,(p obj "free") (+ ,free (* ,(p obj "follow") ,(p obj "hand")))))
      (if (= kind :mallet)
        (list `(def (,(p obj "free") ,(p obj "v"))
                 (mallet-predict (read-history ,(p obj "hx")) (read-history ,(p obj "hv")) hit ,(p obj "drop") ,(p obj "throw"))))
        (list `(def ,(p obj "free") ,free))))))

;; terms: signed (F phi) products already applied to the object, as forms
(def displaced (obj terms)
  (if (empty? terms) (p obj "free")
    (if (get obj :lumped)
      `(+ ,(p obj "free") (* ,(p obj "C") ,(sum-form terms)))
      `(+ ,(p obj "free") (* ,(p obj "G") ,(sum-form terms))))))

;; the library macros take and return values; the patch owns the histories
(def object-commit (obj terms)
  (let ((kind (get obj :kind)) (total (sum-form terms)))
    (if (= kind :mallet)
      (list `(def (,(p obj "next") ,(p obj "next_v"))
               (mallet-advance ,(p obj "free") ,(p obj "v") (* -1 ,total) ,(p obj "C") ,(p obj "drop")))
            `(write-history ,(p obj "hx") ,(p obj "next"))
            `(write-history ,(p obj "hv") ,(p obj "next_v")))
      (list `(def ,(p obj "next") (modal-advance ,(p obj "free")
                                    ,(if (= kind :two-mass) `(* ,(p obj "C") ,total) `(* ,(p obj "G") ,total))))
            `(write-history ,(p obj "s2") (read-history ,(p obj "s1")))
            `(write-history ,(p obj "s1") ,(p obj "next"))))))

;; ── connections ─────────────────────────────────────────────────────────────
;; The force terms connection c applies to object id when its force is the
;; symbol f: +f phi_b on b, -f phi_a on a (phi is 1 for lumped objects).
(def terms-with (c id f)
  (let ((cid (get c :id)))
    (if (= (get c :b) id)
      (list (if (get c :b-lumped) f `(* ,f ,(nm cid "phi_b"))))
      (if (= (get c :a) id)
        (list (if (get c :a-lumped) `(* -1 ,f) `(* -1 ,f ,(nm cid "phi_a"))))
        nil))))
(def terms-on (c id) (terms-with c id (nm (get c :id) "F")))

(def terms-of (conns indices id)
  (concat-all (map (lambda (j) (terms-on (nth conns j) id)) indices)))

;; access forms: phi (modal ends, once) then y and C as the object stands
;; given terms; tag distinguishes Gauss-Seidel sweeps
(def end-forms (c side obj terms tag with-phi)
  (let ((cid (get c :id)) (y (nm cid (str "y_" side tag))) (cs (nm cid (str "C_" side tag))))
    (if (get obj :lumped)
      (list `(def ,y ,(displaced obj terms))
            `(def ,cs ,(p obj "C")))
      (append (if with-phi (shape-defs obj (access-syms cid side obj) (nm cid (str "phi_" side))) nil)
              (list `(def (,y ,cs) (modal-point ,(displaced obj terms) ,(nm cid (str "phi_" side)) ,(p obj "G"))))))))

;; exciters (pluck, felt) keep state, so each is solved once per sample
(def exciter-forms (c)
  (let ((cid (get c :id)) (gap `(- ,(nm cid "y_a") ,(nm cid "y_b"))) (cc `(+ ,(nm cid "C_a") ,(nm cid "C_b"))))
    (if (= (get c :law) :pluck)
      (list `(def ,(nm cid "thr") (* ,(nm cid "thresh") (+ 0.25 (* 0.75 hit_velocity))))
            `(def ,(nm cid "F") (pluck-contact ,gap ,cc hit ,(nm cid "stiff") ,(nm cid "thr"))))
      ;; felt: stiffens with last sample's squash, scaled by a full-speed
      ;; sample of mallet travel
      (list `(make-history ,(nm cid "squash"))
            `(def ,(nm cid "gap") ,gap)
            `(def ,(nm cid "Cs") ,cc)
            `(def ,(nm cid "k") (* ,(nm cid "felt") (+ 1 (* ,(nm cid "hardness")
                                   (/ (read-history ,(nm cid "squash")) (/ ,(nm (get c :a) "speed") samplerate))))))
            `(def ,(nm cid "F") (felt-contact ,(nm cid "gap") ,(nm cid "Cs") ,(nm cid "k")))
            `(write-history ,(nm cid "squash") (max 0 (* -1 (- ,(nm cid "gap") (* ,(nm cid "Cs") ,(nm cid "F"))))))))))

;; a bow's speed envelope and state, emitted once before the sweeps
(def bow-pre-forms (c)
  (let ((cid (get c :id)))
    (list `(make-history ,(nm cid "env_h"))
          `(def ,(nm cid "env") (max (min (+ (read-history ,(nm cid "env_h"))
                                             (gswitch (gt gate 0.5) (/ 1000 (* ,(nm cid "attack") samplerate))
                                                                    (/ -1000 (* ,(nm cid "release") samplerate))))
                                          1) 0))
          `(write-history ,(nm cid "env_h") ,(nm cid "env"))
          `(def ,(nm cid "vmax") (/ ,(nm cid "speed") samplerate))
          `(def ,(nm cid "vb") (* ,(nm cid "vmax") ,(nm cid "env") (+ 0.5 (* 0.5 velocity))))
          `(make-history ,(nm cid "y_prev"))
          `(make-history ,(nm cid "x_prev")))))

(def spring? (c) (= (get c :law) :spring))
(def bow? (c) (= (get c :law) :bow))
(def sweeps? (c) (if (spring? c) true (bow? c)))
(def exciter? (c) (not (sweeps? c)))
(def indices-where (conns pred) (filter (lambda (k) (pred (nth conns k))) (range (len conns))))
(def position-of (xs x) (len (filter (lambda (k) (< (nth xs k) x)) (range (len xs)))))

;; Springs and bows are stateless per call, so when several share objects
;; they are solved by Gauss-Seidel sweeps: each sees the others' latest
;; forces. One sequential pass is not exact when they share an object: stiff
;; springs on a light shared body grew instead of settling, and a bow solved
;; before a bridge read the bridge's push as string velocity and pumped
;; energy in after release. 4 sweeps converge.
(def spring-sweep-forms (patch conns exciters springs sweeps)
  (concat-all
    (map (lambda (s)
           (concat-all
             (map (lambda (k)
                    (let ((c (nth conns k)) (cid (get (nth conns k) :id)) (tag (str s))
                          (others (lambda (id)
                                    (append (terms-of conns exciters id)
                                            (concat-all
                                              (map (lambda (j)
                                                     (if (= j k) nil
                                                       (if (< (position-of springs j) (position-of springs k))
                                                         (terms-with (nth conns j) id (nm (get (nth conns j) :id) (str "F" s)))
                                                         (if (> s 0)
                                                           (terms-with (nth conns j) id (nm (get (nth conns j) :id) (str "F" (- s 1))))
                                                           nil))))
                                                   springs))))))
                      (append (if (get c :a) (end-forms c "a" (find-object patch (get c :a)) (others (get c :a)) tag (= s 0)) nil)
                              (end-forms c "b" (find-object patch (get c :b)) (others (get c :b)) tag (= s 0))
                              (if (bow? c)
                                (list `(def (,(nm cid (str "F" s)) ,(nm cid (str "x" s)))
                                         (bow-contact ,(nm cid (str "y_b" tag)) (read-history ,(nm cid "y_prev"))
                                                      (read-history ,(nm cid "x_prev")) ,(nm cid (str "C_b" tag)) ,(nm cid "vb")
                                                      (* ,(nm cid "slip") ,(nm cid "vmax"))
                                                      ;; the bow lifts as it slows
                                                      (max 0.000001 (* ,(nm cid "pressure") ,(nm cid "env"))))))
                                (list `(def ,(nm cid (str "F" s))
                                         (spring-connection (- ,(nm cid (str "y_a" tag)) ,(nm cid (str "y_b" tag)))
                                                            (+ ,(nm cid (str "C_a" tag)) ,(nm cid (str "C_b" tag)))
                                                            ,(nm cid "stiff")))))
                              (if (= s (- sweeps 1))
                                (append (list `(def ,(nm cid "F") ,(nm cid (str "F" s))))
                                        (if (bow? c) (list `(write-history ,(nm cid "x_prev") ,(nm cid (str "x" s)))) nil))
                                nil))))
                  springs)))
         (range sweeps))))

(def connection-forms (patch conns)
  (let ((exciters (indices-where conns exciter?))
        (springs (indices-where conns sweeps?)))
    (append
      (concat-all
        (map (lambda (k)
               (let ((c (nth conns k)) (before (filter (lambda (j) (< j k)) exciters)))
                 (append (if (get c :a) (end-forms c "a" (find-object patch (get c :a)) (terms-of conns before (get c :a)) "" true) nil)
                         (end-forms c "b" (find-object patch (get c :b)) (terms-of conns before (get c :b)) "" true)
                         (exciter-forms c))))
             exciters))
      (concat-all (map (lambda (k) (bow-pre-forms (nth conns k))) (indices-where conns bow?)))
      (spring-sweep-forms patch conns exciters springs (if (> (len springs) 1) 4 1)))))

(def resolve-connection (patch c)
  (merge c :a-lumped (if (get c :a) (get (find-object patch (get c :a)) :lumped) false)
           :b-lumped (get (find-object patch (get c :b)) :lumped)))

(def connection-params (patch c)
  (append (get c :params)
          (if (get c :a) (access-params (get c :id) "a" (find-object patch (get c :a)) (get c :a-at)) nil)
          (access-params (get c :id) "b" (find-object patch (get c :b)) (get c :b-at))))

;; ── listening ───────────────────────────────────────────────────────────────
;; velocity at a point, normalised by the object's reference frequency and
;; mass so that a light string and a heavy plate mix at similar levels
(def mic (k) (symbol "mic" (+ k 1)))
(def listen-forms (patch l k)
  (let ((obj (find-object patch (get l :obj))) (lid (mic k)))
    (append (shape-defs obj (access-syms lid "mic" obj) (nm lid "phi"))
     (list
          `(make-history ,(nm lid "prev"))
          `(def ,(nm lid "y") (sum (* ,(p obj "next") ,(nm lid "phi"))))
          `(def ,(nm lid "v") (/ (* (- ,(nm lid "y") (read-history ,(nm lid "prev"))) ,(p obj "mass")) ,(p obj "wref")))
          `(write-history ,(nm lid "prev") ,(nm lid "y"))))))

(def mic-gain-param (patch k)
  (pspec (mic k) "gain" (get (nth (get patch :listens) k) :gain) 0 4 :title (str (source (mic k)) " gain")))
(def mic-at-params (patch k &key (label nil))
  (let ((l (nth (get patch :listens) k)))
    (access-params (mic k) "mic" (find-object patch (get l :obj)) (get l :at) :label label)))
(def out-gain-param () (pspec 'out "gain" 0.5 0 2 :title "Gain"))

(def listen-params (patch k)
  (append (mic-at-params patch k) (list (mic-gain-param patch k))))

;; Level normalisation by what drives the patch (its first pluck, felt or bow),
;; so the output sits near the same level whatever the exciter's settings: a
;; pluck's displacement scales with its release force, a mallet's with its
;; throw speed, (sub-linearly) its mass, and falls with the struck pitch; a
;; bowed point moves at about the bow's speed.
(def excite-norm (patch conns)
  (let ((drivers (filter (lambda (c) (not (spring? c))) conns)))
    (if (empty? drivers) 1
      (let ((c (first drivers)))
        (if (bow? c)
          ;; velocity relative to the bow's speed
          `(* 0.3 (/ ,(nm (get c :b) "wref") ,(nm (get c :id) "vmax")))
        (if (= (get c :law) :pluck)
          `(/ 0.5 ,(nm (get c :id) "thresh"))
          ;; (wref / C3)^1.6 flattens the mallet's -9.6 dB/octave
          `(* (/ samplerate (* ,(nm (get c :a) "speed") 10))
              (pow (/ 0.01 ,(nm (get c :a) "mass")) 0.45)
              (pow (/ ,(nm (get c :b) "wref") 0.0171) 1.6))))))))

;; ── the whole program ───────────────────────────────────────────────────────
(def preamble ()
  (list '(def gate (in 1 @name gate))
        '(def pitch (in 2 @name pitch))
        '(def velocity (in 3 @name velocity))
        '(def trigger (in 4 @name trigger))
        '(def clock (in 5 @name clock))
        '(defmacro onset-of (trigger)
           (make-history previous)
           (def high (gt trigger 0.5))
           (def rising (* high (lte (read-history previous) 0.5)))
           (write-history previous high)
           rising)
        '(defmacro seconds-since (onset)
           (make-history elapsed)
           (make-history armed)
           (def played (max onset (read-history armed)))
           (write-history armed played)
           (def samples (gswitch onset 0 (min (* 8 samplerate) (+ 1 (read-history elapsed)))))
           (write-history elapsed samples)
           (gswitch played (/ samples samplerate) 8))
        '(def hit (onset-of trigger))
        '(def age (seconds-since hit))
        '(def hit_velocity (latch velocity hit))
        '(def f0 (max pitch 20))))

(def library-imports ()
  (map (lambda (name) (list 'use-defmacro name))
       '(modal-material stiff-string-freqs rect-plate rect-plate-shape circ-membrane
         modal-band modal-loss modal-coefs modal-predict modal-point modal-advance
         two-mass-coefs mallet-predict mallet-advance pluck-contact spring-connection
         felt-contact bow-contact voice-amp)))

(def all-params (patch conns)
  (concat-all (list (concat-all (map (lambda (o) (get o :params)) (get patch :objects)))
                    (concat-all (map (lambda (c) (connection-params patch c)) conns))
                    (concat-all (map (lambda (k) (listen-params patch k)) (range (len (get patch :listens)))))
                    (list (out-gain-param)))))

(def patch-forms (patch)
  (let ((conns (map (lambda (c) (resolve-connection patch c)) (get patch :connections)))
        (objects (get patch :objects))
        (listens (get patch :listens)))
    (concat-all
      (list (preamble)
            (map param-form (all-params patch conns))
            (library-imports)
            (concat-all (map object-setup objects))
            (concat-all (map object-predict objects))
            (connection-forms patch conns)
            (concat-all (map (lambda (o) (object-commit o (terms-of conns (range (len conns)) (get o :id)))) objects))
            ;; a bow needs its point's true position after every force
            (concat-all (map (lambda (c) (if (bow? c)
                                           (list `(write-history ,(nm (get c :id) "y_prev")
                                                    (sum (* ,(p (find-object patch (get c :b)) "next") ,(nm (get c :id) "phi_b")))))
                                           nil))
                             conns))
            (concat-all (map (lambda (k) (listen-forms patch (nth listens k) k)) (range (len listens))))
            (list `(def mix ,(sum-form (map (lambda (k) `(* ,(nm (mic k) "v") ,(nm (mic k) "gain")))
                                            (range (len listens)))))
                  `(def raw (* mix ,(excite-norm patch conns) ,(get patch :level) out_gain))
                  '(make-history dcx)
                  '(make-history dcy)
                  '(def dc-out (+ (- raw (read-history dcx)) (* 0.9995 (read-history dcy))))
                  '(write-history dcx raw)
                  '(write-history dcy dc-out)
                  '(def voice_out (tanh dc-out))
                  '(out voice_out 1 @name audio)
                  '(out (voice-amp (max (gt gate 0.5) (lt age 0.25)) voice_out voice_out) 2 @name amp @amp true))))))

;; The instrument's dsp.lisp source.
(def dsp (patch)
  (str "; generated by eseq.modal; edit the patch script, not this file\n"
       (pp-forms (patch-forms patch))))

;; ── panel: ui.lisp in the style of .local/instruments/modalys-pluck ───────
;; One section per object and connection, then OUTPUT (the mics and the
;; gain). Each section gets a panel with its first three params as knobs;
;; the screen in the middle draws the patch (click an object or connection
;; to select it) and lists every param of the selected section.

;; the mics in the OUTPUT section: every gain first, then where each listens
(def output-params (patch)
  (let ((ks (range (len (get patch :listens)))))
    (append (map (lambda (k) (mic-gain-param patch k)) ks)
            (list (out-gain-param))
            (concat-all (map (lambda (k) (mic-at-params patch k :label (source (mic k)))) ks)))))

(def sections (patch)
  (let ((conns (map (lambda (c) (resolve-connection patch c)) (get patch :connections))))
    (append (map (lambda (o) (dict :title (str (get o :title) " " (source (get o :id))) :tab (source (get o :id))
                                   :params (get o :params)))
                 (get patch :objects))
            (map (lambda (c) (dict :title (str (get c :title) " " (source (get c :id))) :tab (source (get c :id))
                                   :params (connection-params patch c)))
                 conns)
            (list (dict :title "OUTPUT" :tab "out" :params (output-params patch))))))

(def section-of (patch id)
  (let ((ids (append (map (lambda (o) (get o :id)) (get patch :objects))
                     (map (lambda (c) (get c :id)) (get patch :connections)))))
    (first (filter (lambda (k) (= (nth ids k) id)) (range (len ids))))))

(def take (xs n) (map (lambda (i) (nth xs i)) (range (min n (len xs)))))

;; Helpers every generated panel shares (mx-*: kept apart from the user's
;; own panel code when they edit the result).
(def ui-helpers ()
  '((def mx-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
    (def mx-section () eseq.vanilla/custom-ui-selected-section)
    (def mx-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
    (def mx-select (section) (eseq.effects.custom-ui-sections/ui-section-select-callback section))
    (def mx-knob (section name title decimals taper)
      (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 5.6 3.45 2.15
        (mx-c) decimals taper :widget-knob-track 8.5 8 :center))
    (def mx-label (title width)
      (box :width width :height 0.72 :background-color (mx-c)
        (label title :width width :height 0.72 :h-align :center :v-align :center
          :font-size 8.2 :color :black :bg :transparent)))
    (def mx-panel (section title width body)
      (box :width width :height 4.8 :padding 0.18
        :background-color (if (= (mx-section) section) :instrument-panel-bg :instrument-group-bg)
        :debug-name (str "modal-panel-" section)
        :on-click (mx-select section)
        (v-stack :gap 0.14 (mx-label title (min width 9)) body)))
    (def mx-option (section name title options)
      (let ((p (mx-p name)) (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-opt-mod-" name)
          (box :width 6.6 :height 3.45 :v-align :center
            (v-stack :width 6.6 :height 1.4 :gap 0.12
              (label title :width 6.6 :height 0.5 :h-align :center :v-align :center
                :font-size 8.5 :bg :transparent)
              (dropdown :debug-name (str "modal-" name) :width 6.6 :height 0.7 :font-size 7.2
                :options options
                :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :text-color (mx-c) :chevron-color (mx-c) :bg-color :instrument-control-bg :border-color :transparent
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (lambda (v)
                  (do
                    (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                      (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
    (def mx-num (section name title width decimals)
      (let ((p (mx-p name))
            (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-mod-" name)
          (subtree :key (str "modal-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
              (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
            (v-stack :width width :height 1.15 :gap 0.08
              (label title :height 0.5 :v-align :center :font-size 9.5 :color ink :bg :transparent)
              (number-picker :width width :height 0.55 :noui true :font-size 10 :decimals decimals
                :unit (eseq.effects.custom-ui-runtime/custom-ui-param-control-unit p)
                :step (pow 10 (- 0 decimals))
                :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
                :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
                :text-align :left
                :text-color ink :edit-color ink :cursor-color ink
                :plock-style :underline
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))
    ;; the screen's patch drawing: objects as chips, connections as links
    (def mx-chip (section title)
      (button title :width (+ 1.4 (* 0.62 (len title))) :height 0.8 :padding 0 :font-size 8.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :black
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))
    (def mx-link (section title)
      (button title :width (+ 1 (* 0.55 (len title))) :height 0.8 :padding 0 :font-size 7.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black :transparent)
        :on-click (mx-select section)))
    (def mx-tab (section title width)
      (button title :width width :height 0.8 :padding 0 :font-size 8
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))))

;; the host addresses a grouped param as group.name
(def host-name (p) (str (source (get p :group)) "." (source (get p :sym))))

(def control-form (k p)
  (if (get p :options)
    `(mx-option ,k ,(host-name p) ,(get p :title) (quote ,(get p :options)))
    `(mx-knob ,k ,(host-name p) ,(get p :title) ,(get p :dec) ,(get p :taper))))

(def num-form (k p width)
  `(mx-num ,k ,(host-name p) ,(get p :title) ,width ,(get p :dec)))

(def panel-form (k sec knobs width)
  `(mx-panel ,k ,(get sec :title) ,width
     (h-stack :gap 0.15 ,@(map (lambda (p) (control-form k p)) (take (get sec :params) knobs)))))

;; one line of the drawing per connection: a -law id-> b (a bow has no a)
(def link-row (patch c)
  (let ((k (section-of patch (get c :id))) (law (law-name c)))
    `(h-stack :gap 0.1 :align :center
       ,@(if (get c :a)
           (list `(mx-chip ,(section-of patch (get c :a)) ,(source (get c :a))))
           (list))
       (mx-link ,k ,(str "-" law " " (source (get c :id)) "->"))
       (mx-chip ,(section-of patch (get c :b)) ,(source (get c :b))))))

(def mic-rows (patch out)
  (map (lambda (ks)
         `(h-stack :gap 0.1 :align :center
            ,@(concat-all
                (map (lambda (k)
                       (let ((l (nth (get patch :listens) k)))
                         (list `(mx-link ,out ,(str (source (mic k)) " @"))
                               `(mx-chip ,(section-of patch (get l :obj)) ,(source (get l :obj))))))
                     ks))))
       (chunks (range (len (get patch :listens))) 2)))

;; the drawing's rows, in two columns once there are more than four
(def drawing-form (patch out)
  (let ((rows (append (map (lambda (c) (link-row patch c)) (get patch :connections))
                      (mic-rows patch out))))
    (if (> (len rows) 4)
      `(h-stack :gap 0.6 :align :start
         ,@(map (lambda (col) `(v-stack :gap 0.12 ,@col)) (chunks rows (ceil (/ (len rows) 2)))))
      `(v-stack :gap 0.12 ,@rows))))

;; every param of the selected section, six to a row
(def page-form (k sec)
  `(v-stack :gap 0.12
     ,@(map (lambda (row) `(h-stack :gap 0.35 ,@(map (lambda (p) (num-form k p 5.3)) row)))
            (chunks (filter (lambda (p) (not (get p :options))) (get sec :params)) 6))))

(def pages-from (secs k)
  (if (= k (- (len secs) 1))
    (page-form k (nth secs k))
    `(if (= (mx-section) ,k) ,(page-form k (nth secs k)) ,(pages-from secs (+ k 1)))))

(def display-form (patch secs)
  (let ((out (- (len secs) 1))
        (tab-w (/ (- 35.2 (* 0.18 (- (len secs) 1))) (len secs))))
    `(box :debug-name "modal-display" :part "instrument/screen" :width 35.8 :height 9.8 :padding 0.3
          :background-color (mx-c)
       (v-stack :gap 0.15
         (label (str ,(str (get patch :name) " / ") (nth (quote ,(map (lambda (s) (get s :title)) secs)) (mx-section)))
           :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (mx-c) :bg :black)
         (box :width 35.2 :height 4.1 ,(drawing-form patch out))
         (box :width 35.2 :height 2.6 ,(pages-from secs 0))
         (h-stack :gap 0.18
           ,@(map (lambda (k) `(mx-tab ,k ,(get (nth secs k) :tab) ,tab-w)) (range (len secs))))))))

;; objects' panels to the left of the screen, connections' and OUTPUT's to
;; the right, two to a column; narrower two-knob panels once it gets wide
(def ui-forms (patch)
  (let ((secs (sections patch))
        (n-obj (len (get patch :objects)))
        (columns (+ (ceil (/ n-obj 2)) (ceil (/ (- (len secs) n-obj) 2))))
        (knobs (if (> columns 3) 2 3))
        (width (if (> columns 3) 12.6 18.7))
        (panels (lambda (from to)
                  (map (lambda (pair) `(v-stack :gap 0.2 ,@pair))
                       (chunks (map (lambda (k) (panel-form k (nth secs k) knobs width)) (range from to)) 2)))))
    (append (ui-helpers)
            (list `(defsynth-ui
                     (h-stack :height 9.8 :gap 0.3 :align :start
                       ,@(panels 0 n-obj)
                       ,(display-form patch secs)
                       ,@(panels n-obj (len secs))))))))

;; The instrument's ui.lisp source.
(def ui (patch)
  (str ";; generated by eseq.modal; edit the patch script, not this file\n"
       (pp-forms (ui-forms patch))))

;; What a host runs on a script's patch: (dict :name :dsp :ui), or
;; (dict :errors messages) when check finds problems.
(def generate (patch)
  (let ((errors (check patch)))
    (if (empty? errors)
      (dict :name (get patch :name) :dsp (dsp patch) :ui (ui patch))
      (dict :errors errors))))
