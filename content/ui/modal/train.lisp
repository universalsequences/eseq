;; eseq.modal.train — trainable modal voices, the feed-forward cousin of eseq.modal.
;;
;; A voice is a list of PARTS (decaying modes, strike sines, noise bands, a mode
;; template, a second stroke, chain stages). Every part is feed-forward: sums of
;; decaying oscillators and fixed-filter noise, gains applied per hit, nothing
;; feeds back, so `DGenLisp train-dataset` can fit every sample and frame in
;; parallel. A 4-number latent is decoded per hit into the parts' row segments.
;;
;;   (import eseq.modal.train :as t)
;;   (def v (t/voice "break-snare"
;;            :parts (list (t/modes 'head :count 16 :glide true :tail true)
;;                         (t/strikes 'stick)
;;                         (t/shaper 'body) (t/clipper 'clip) (t/record-lp 'rec) (t/timing 'time)
;;                         (t/bands 'wires) ...)))
;;   (t/emit v "out")   ; writes out/voice.lisp (training), out/morph.lisp, out/layout.json
;;
;; Each part is a dict made by a kind function below. Its :segments are its
;; `row-layout` lines (in part order they ARE the decoder's output layout), and
;; its :build is called twice, once per variant:
;;   :static  the training voice (train-dataset's --patch; build_instrument.py
;;            turns a trained run into the instrument)
;;   :morph   the Morph instrument's voice: decoded values may change during a hit
;;            (ramped / falling / at-hit / decaying, see morph-core-forms)
;; and returns a dict of form lists, each spliced into one section of the voice:
;;   :prelude  constants and macros, right after the decoder
;;   :hit      per-hit controls (static: at-hit; morph: ramped / falling)
;;   :fixed    morph only: controls fixed from START at the hit
;;   :stroke   per-sample forms other parts read (a second stroke's t2 / on2)
;;   :dsp      per-sample forms
;;   :body     terms summed into the body bus (the shaper's input)
;;   :ms       envelopes whose squares give the body's mean square (shaper DC)
;;   :drum     terms summed into the drum bus (the clipper's input)
;;   :post     terms added after the clipper
;;   :attach   (bank parts only read this) a hiss-like envelope term on a bank
;;
;; Codegen is quasiquoted forms printed by pp-forms, as in eseq.modal. A part's
;; internal names are prefixed with its id; segment names default to
;; <id>_<field> and can be given with :names (field order) to load old rows.

(module eseq.modal.train)

(import eseq.gen :as gen :refer (concat-all member? any? form-atoms))

(export voice modes strikes shaper clipper record-lp timing bands hiss crackle events
        template wash restrike voice-forms morph-forms layout-json emit segment-count
        knob-table data seed refine pick pretrain path recipe ship experiment emit-experiment to-json
        variant strongest modes-of strings rattle)

;; ── helpers ───────────────────────────────────────────────────────────────

;; A symbol <id>-<name>: a part's internal name.
(def s (p name) (symbol (source (get p :id)) "-" name))

;; The DGenLisp range literal `text` (e.g. "[:,0:16]"): pp-forms prints a symbol
;; verbatim, and a literal comma inside quasiquote would read as unquote.
(def rng (text) (symbol text))

;; a's entries overridden by b's (merge takes key value pairs; this takes a dict).
(def merged (a b) (reduce (lambda (acc k) (merge acc k (get b k))) a (keys b)))

;; `(def NAME VALUE)`.
(def df (name value) `(def ,name ,value))

;; Sum of terms: the term alone when there is one, 0 when none.
(def plus (terms) (if (empty? terms) 0 (if (= (len terms) 1) (first terms) `(+ ,@terms))))

;; The segment symbol of field k of part p (its :names, else <id>_<field>).
(def seg-sym (p fields k)
  (let ((names (get p :names)))
    (if (and names (< k (len names))) (nth names k)
      (symbol (source (get p :id)) "_" (first (nth fields k))))))

;; A part's segment table: (field-name segment-symbol) pairs (dict keys must be
;; keywords, and field names are strings).
(def seg-table (p fields)
  (map (lambda (k) (list (first (nth fields k)) (seg-sym p fields k))) (range (len fields))))

(def seg-lookup (p name)
  (let ((hits (filter (lambda (e) (= (first e) name)) (get p :seg))))
    (if (empty? hits) (error (str (source (get p :id)) ": no segment field " name)) (nth (first hits) 1))))

(def seg-forms (p fields)
  (map (lambda (k) (let ((f (nth fields k)))
                     (cons (seg-sym p fields k) (rest f))))
       (range (len fields))))

;; `(decoded SEG)` for field `name` of part p.
(def D (p name) `(decoded ,(seg-lookup p name)))

;; A part dict: kind, id, segments (built from fields), options, build.
(def part (kind id fields opts build)
  (let ((p (merged opts (dict :kind kind :id id :fields fields :build build))))
    (merged p (dict :seg (seg-table p fields) :segments (seg-forms p fields)))))

;; Alias guard: 1 below 0.44 sr, fading to 0 at 0.48 sr.
(def alias-form (hz) `(clip (/ (- (* 0.48 samplerate) ,hz) (* 0.04 samplerate)) 0 1))

;; `[1 n]` shape literal.
(def shape (n) (rng (str "[1 " n "]")))

;; The first part of `kind` in ctx (or the one named id).
(def find-part (ctx kind id)
  (let ((ps (filter (lambda (p) (and (= (get p :kind) kind) (or (= id nil) (= (get p :id) id))))
                    (get ctx :parts))))
    (if (empty? ps) nil (first ps))))

;; The second stroke's names, or nil when the voice has none.
(def stroke-of (ctx) (find-part ctx :restrike nil))

;; ── modes: decaying sines ─────────────────────────────────────────────────
;; :count n  :follow-note (x tune x note)  :glide (one shared pitch glide, the
;; drum head's tension)  :tail (slot 0 gets a second, slow decay)
;; :rise :shared (one learned attack) or :instant (0.1 ms)  :strokes (a second
;; stroke re-excites them)  :bus :body or :drum  :knob a level knob symbol
;; :ranges (dict :f (lo hi) :amp (lo hi) :dec (lo hi)) segment ranges
(def modes (id &key (count 16) (follow-note true) (glide false) (tail false) (rise :instant)
                   (strokes false) (bus :drum) (knob nil) (ranges nil) (names nil))
  (let ((rg (lambda (k lo hi) (let ((r (if ranges (get ranges k) nil))) (if r r (list lo hi))))))
    (let ((fields (concat-all
                    (list (list (append (list "f" count 'log) (rg :f 30 16000))
                                (append (list "amp" count 'log) (rg :amp 0.000001 20))
                                (append (list "dec" count 'log) (rg :dec 0.001 4)))
                          (if (= rise :shared) (list (list "rise" 1 'log 0.00001 0.05)) (list))
                          (list (list "ph" count 'raw))
                          (if glide (list (list "g" 1 'lin 0 1) (list "td" 1 'log 0.0005 0.2)) (list))
                          (if tail (list (list "tamp" 1 'log 0.000001 4) (list "tdec" 1 'log 0.005 4)) (list))))))
      (part :modes id fields
            (dict :count count :follow-note follow-note :glide glide :tail tail :rise rise
                  :strokes strokes :bus bus :knob knob :names names)
            modes-build))))

(def modes-build (p ctx v)
  (let ((n (get p :count)) (follow (get p :follow-note)) (glide (get p :glide)) (tail (get p :tail))
        (shared (= (get p :rise) :shared)) (st (if (get p :strokes) (stroke-of ctx) nil))
        (knobs (if (get p :knob) (list (get p :knob)) (list)))
        (F (s p "F")) (iT (s p "iT")) (FGT (s p "FGT")) (G (s p "G")) (iD (s p "iD")) (iR (s p "iR"))
        (PH (s p "PH")) (iTD (s p "iTD")) (A (s p "A")) (TA (s p "TA")) (FHZ (s p "fhz"))
        (SLOT0 (s p "slot0")) (E1 (s p "e1")) (ENV (s p "env")) (OUT (s p "out")))
    (let ((fhz (if follow FHZ (D p "f")))
          (slot0 (if tail (list (df SLOT0 `(tensor @shape ,(shape n) @data ,(rng (str "[1" (apply str (map (lambda (k) " 0") (range (- n 1)))) "]"))))) (list)))
          (outs (if (= (get p :bus) :body) (dict :body (list OUT) :ms (list ENV)) (dict :drum (list OUT)))))
     (if (= v :static)
      (merged outs (dict
        :prelude slot0
        :hit (concat-all (list
               (list (df F `(at-hit ,(if follow `(* ratio ,(D p "f")) (D p "f")))))
               (if glide (list (df iT `(at-hit (/ 1 ,(D p "td"))))
                               (df FGT `(at-hit (* ,@(if follow '(ratio) '()) ,(D p "f") bendv ,(D p "g") ,(D p "td")))))
                 (list))
               (list (df iD `(at-hit (/ 1 (* decay ,(D p "dec"))))))
               (if shared (list (df iR `(at-hit (/ 1 (* attack ,(D p "rise")))))) (list))
               (list (df PH `(at-hit ,(D p "ph"))))
               (if tail (list (df iTD `(at-hit (/ 1 (* decay ,(D p "tdec")))))) (list))
               (if follow (list (df FHZ `(* ratio ,(D p "f")))) (list))
               (list (df A `(at-hit (* ,@knobs ,(D p "amp") ,(alias-form fhz) (rlg-hit ,fhz)))))
               (if tail (list (df TA `(at-hit (* ,SLOT0 ,(D p "tamp") (rlg-hit ,fhz))))) (list))))
        :dsp (concat-all (list
               (if glide (list (df E1 `(exp (* -1 t ,iT)))) (list))
               (list (df ENV
                 (if shared
                   `(* (- 1 (exp (* -1 t ,iR)))
                       ,(plus (append (list `(* ,A (exp (* -1 t ,iD))))
                                      (if tail (list `(* ,TA (exp (* -1 t ,iTD)))) (list)))))
                   (if st
                     `(* ,A (+ (* (exp (* -1 t ,iD)) (- 1 (exp (* -10000 t))))
                               (* ,(s st "on2") (exp (* -1 ,(s st "t2") ,iD)))))
                     `(* ,A (exp (* -1 t ,iD)) (- 1 (exp (* -10000 t))))))))
               (list (df OUT `(sum-axis (* ,ENV (sin ,(if glide `(+ (* twopi (+ (* ,F t) (* ,FGT (- 1 ,E1)))) ,PH)
                                                             `(+ (* twopi ,F t) ,PH))))
                                        @axis 1 @keepdims true)))))))
      ;; morph: frequency, glide and decays ramp (a free partial's frequency is
      ;; START's: a cymbal partial does not glide); levels fall; rise and phase
      ;; are START's
      (let ((fhzd (if follow (s p "fhzd") (D p "f"))) (DN (s p "dn")) (TDN (s p "tdn")) (DN2 (s p "dn2"))
            (PHH (s p "ph-h")) (PHS (s p "phs")) (TN (s p "tn")) (K1 (s p "k1")) (INC (s p "inc")) (PHN (s p "ph-next")))
       (merged outs (dict
        :prelude slot0
        :hit (concat-all (list
               (list (df F (if follow `(ramped (* ratio ,(D p "f")) ,(shape n)) `(at-hit ,(D p "f")))))
               (if glide (list (df G `(ramped (* bendv ,(D p "g")) ,(shape 1)))
                               (df iT `(ramped (/ 1 ,(D p "td")) ,(shape 1))))
                 (list))
               (list (df iD `(ramped (/ 1 (* decay ,(D p "dec"))) ,(shape n))))
               (if tail (list (df iTD `(ramped (/ 1 (* decay ,(D p "tdec"))) ,(shape 1)))) (list))
               (if follow (list (df fhzd `(* ratio ,(D p "f")))) (list))
               (list (df A `(falling (* ,@knobs ,(D p "amp") ,(alias-form fhzd) (rlg-at ,fhzd rlm-d rlf-d)) ,(shape n))))
               (if tail (list (df TA `(falling (* ,(D p "tamp") (rlg-at (shrink ,fhzd @ranges ,(rng "[:,0:1]")) rlm-d rlf-d)) ,(shape 1))))
                 (list))))
        :fixed (concat-all (list
               (if shared (list (df iR `(at-hit (/ 1 (* attack ,(D p "rise")))))) (list))
               (list (df PH `(at-hit ,(D p "ph"))))))
        :dsp (concat-all (list
               (if glide (list (df E1 `(decaying ,iT T0 ,(shape 1)))) (list))
               (list (df DN `(decaying ,iD T0 ,(shape n))))
               (if tail (list (df TDN `(decaying ,iTD T0 ,(shape 1)))) (list))
               (if st (list (df DN2 `(decaying ,iD (+ T0 ,(s st "MRT")) ,(shape n)))) (list))
               (if glide
                 ;; phase: integrated while active; before that, the closed form at the next sample
                 (list `(make-tensor-history ,PHH @shape ,(shape n))
                       (df TN '(max 0 (- (+ tm isr) T0)))
                       (df PHS `(gswitch hit (+ ,PH (* twopi (+ (* ,F t) (* ,F ,G (/ 1 ,iT) (- 1 ,E1))))) (read-tensor-history ,PHH)))
                       (df K1 `(exp (* -1 ,iT isr)))
                       (df INC `(* twopi (+ (* ,F isr) (/ (* ,F ,G ,E1 (- 1 ,K1)) ,iT))))
                       (df PHN `(gswitch act (+ ,PHS ,INC) (+ ,PH (* twopi (+ (* ,F ,TN) (* ,F ,G (/ 1 ,iT) (- 1 (exp (* -1 ,TN ,iT)))))))))
                       `(write-tensor-history ,PHH (- ,PHN (* twopi (floor (/ ,PHN twopi))))))
                 (list))
               (list (df ENV
                 (if shared
                   `(* (- 1 (exp (* -1 t ,iR)))
                       ,(plus (append (list `(* ,A ,DN)) (if tail (list `(* ,SLOT0 ,TA ,TDN)) (list)))))
                   (if st
                     `(* ,A (+ (* ,DN (- 1 (exp (* -10000 t)))) (* ,(s st "on2") ,DN2)))
                     `(* ,A ,DN (- 1 (exp (* -10000 t))))))))
               (list (df OUT `(sum-axis (* ,ENV (sin ,(if glide PHS `(+ (* twopi ,F t) ,PH)))) @axis 1 @keepdims true))))))))))))

;; ── strike sines: short gliding sines (the stick) ─────────────────────────
(def strikes (id &key (count 4) (names nil))
  (part :strikes id
        (list (list "xf" count 'log 100 16000) (list "xg" count 'lin 0 4) (list "xtd" count 'log 0.0003 0.05)
              (list "xamp" count 'log 0.000001 4) (list "xdec" count 'log 0.0002 0.05) (list "xph" count 'raw))
        (dict :count count :names names) strikes-build))

(def strikes-build (p ctx v)
  (let ((XF (s p "XF")) (XFGT (s p "XFGT")) (iXT (s p "iXT")) (XA (s p "XA")) (iXD (s p "iXD")) (XPH (s p "XPH"))
        (XE (s p "xe")) (XHZ (s p "xhz")) (XAL (s p "xalias")) (XENV (s p "xenv")) (OUT (s p "out")))
    (let ((hit (list (df XF `(at-hit (* ratio ,(D p "xf"))))
                     (df XFGT `(at-hit (* ratio ,(D p "xf") bendv ,(D p "xg") ,(D p "xtd"))))
                     (df iXT `(at-hit (/ 1 ,(D p "xtd"))))
                     (df XA `(at-hit (* punch vsnap ,(D p "xamp"))))
                     (df iXD `(at-hit (/ 1 (* attack ,(D p "xdec")))))
                     (df XPH `(at-hit ,(D p "xph"))))))
      (dict (if (= v :static) :hit :fixed) hit
            :dsp (list (df XE `(exp (* -1 t ,iXT)))
                       (df XHZ `(* ,XF (+ 1 (/ (* ,XFGT ,iXT ,XE) ,XF))))
                       (df XAL (alias-form XHZ))
                       (df XENV `(* ,XA ,XAL (rlg ,XHZ) (exp (* -1 t ,iXD)) (- 1 (exp (* -10000 t)))))
                       (df OUT `(sum-axis (* ,XENV (sin (+ (* twopi (+ (* ,XF t) (* ,XFGT (- 1 ,XE)))) ,XPH))) @axis 1 @keepdims true)))
            :body (list OUT) :ms (list XENV)))))

;; ── chain: body shaper, recording clipper, record low-pass, timing ────────

;; Body shaper on the body bus: odd (tanh) + even (square) at a pre-gain, back
;; at unit gain; the even term's DC removed in closed form.
(def shaper (id &key (names nil))
  (part :shaper id (list (list "sh_in" 1 'log 0.05 20) (list "sh_odd" 1 'lin 0 1) (list "sh_even" 1 'lin -1 1))
        (dict :names names) shaper-build))

(def shaper-build (p ctx v)
  (let ((SHI (s p "SHI")) (SHO (s p "SHO")) (SHE (s p "SHE")) (BODY (s p "body")) (MS (s p "ms"))
        (SX (s p "sx")) (OUT (s p "out")))
    (let ((ms (get ctx :ms)) (body (get ctx :body)))
      (dict :hit (if (= v :static)
                   (list (df SHI `(at-hit (* drive ,(D p "sh_in")))) (df SHO `(at-hit (* shape ,(D p "sh_odd"))))
                         (df SHE `(at-hit (* shape ,(D p "sh_even")))))
                   (list (df SHI `(ramped (* drive ,(D p "sh_in")) ,(shape 1))) (df SHO `(ramped (* shape ,(D p "sh_odd")) ,(shape 1)))
                         (df SHE `(ramped (* shape ,(D p "sh_even")) ,(shape 1)))))
            :dsp (list (df BODY (plus body))
                       (df MS `(* 0.5 ,(plus (map (lambda (e) `(sum-axis (* ,e ,e) @axis 1 @keepdims true)) ms))))
                       (df SX `(* ,SHI ,BODY))
                       (df OUT `(/ (+ (* (- 1 ,SHO) ,SX) (* ,SHO (tanh ,SX)) (* ,SHE (- (* ,SX ,SX) (* ,SHI ,SHI ,MS)))) ,SHI)))
            :drum (list OUT)))))

;; Asymmetric recording clipper over the drum bus (offset, separate ceilings,
;; wet mix). The assembler calls it on the bus; it defines `drum`.
(def clipper (id &key (names nil))
  (part :clipper id (list (list "c_pos" 1 'log 0.05 4) (list "c_neg" 1 'lin 0.3 1.5) (list "c_off" 1 'lin -0.3 0.3)
                          (list "c_mix" 1 'lin 0 1))
        (dict :names names) clipper-build))

(def clipper-build (p ctx v)
  (let ((CP (s p "CP")) (CN (s p "CN")) (COFF (s p "COFF")) (COFF0 (s p "COFF0")) (CMIX (s p "CMIX")))
    (dict :hit (if (= v :static)
                 (list (df CP `(at-hit ,(D p "c_pos")))
                       (df CN `(at-hit (* ,(D p "c_pos") ,(D p "c_neg"))))
                       (df COFF `(at-hit ,(D p "c_off")))
                       (df (s p "coff-p") `(* ,(D p "c_pos") (tanh (/ ,(D p "c_off") ,(D p "c_pos")))))
                       (df (s p "coff-n") `(* ,(D p "c_pos") ,(D p "c_neg") (tanh (/ ,(D p "c_off") (* ,(D p "c_pos") ,(D p "c_neg"))))))
                       (df COFF0 `(at-hit (gswitch (gt ,(D p "c_off") 0) ,(s p "coff-p") ,(s p "coff-n"))))
                       (df CMIX `(at-hit ,(D p "c_mix"))))
                 (list (df CP `(ramped ,(D p "c_pos") ,(shape 1)))
                       (df CN `(ramped (* ,(D p "c_pos") ,(D p "c_neg")) ,(shape 1)))
                       (df COFF `(ramped ,(D p "c_off") ,(shape 1)))
                       (df CMIX `(ramped ,(D p "c_mix") ,(shape 1)))))
          :mix (lambda (dr)
                 (append (list (df 'cx `(+ ,dr ,COFF))
                               (df 'cpos `(* ,CP (tanh (/ (max cx 0) ,CP))))
                               (df 'cneg `(* ,CN (tanh (/ (min cx 0) ,CN)))))
                         (if (= v :static)
                           (list (df 'clipped `(- (+ cpos cneg) ,COFF0)))
                           (list (df 'coff0 `(gswitch (gt ,COFF 0) (* ,CP (tanh (/ ,COFF ,CP))) (* ,CN (tanh (/ ,COFF ,CN)))))
                                 (df 'clipped '(- (+ cpos cneg) coff0))))
                         (list (df 'drum `(+ (* (- 1 ,CMIX) ,dr) (* ,CMIX clipped)))))))))

;; Record low-pass: two one-poles at rl_hz as their exact sinusoid gain
;; 1/(1+(f/fc)^2), mixed with dry by rl_mix. Defines the macros every level uses:
;; static `rlg` (per sample) and `rlg-hit` (from the decoded row); morph `rlg`
;; and `rlg-at` (ramped corner and mix). Without this part they are 1.
(def record-lp (id &key (names nil))
  (part :record-lp id (list (list "rl_hz" 1 'log 300 20000) (list "rl_mix" 1 'lin 0 1)) (dict :names names) record-lp-build))

(def record-lp-build (p ctx v)
  (let ((hz (D p "rl_hz")) (mix (D p "rl_mix")))
    (if (= v :static)
      (dict :prelude (list `(defmacro rlg (fhz) (+ (- 1 RLM) (/ RLM (+ 1 (* (/ fhz RLF) (/ fhz RLF))))))
                           `(defmacro rlg-hit (fhz) (+ (- 1 ,mix) (/ ,mix (+ 1 (* (/ fhz ,hz) (/ fhz ,hz)))))))
            :hit (list (df 'RLF `(at-hit ,hz)) (df 'RLM `(at-hit ,mix))))
      (dict :prelude (list (df 'rlm-d mix) (df 'rlf-d hz)
                           '(defmacro rlg-at (fhz rlm rlf) (+ (- 1 rlm) (/ rlm (+ 1 (* (/ fhz rlf) (/ fhz rlf))))))
                           '(defmacro rlg (fhz) (+ (- 1 RLM) (/ RLM (+ 1 (* (/ fhz RLF) (/ fhz RLF)))))))
            :hit (list (df 'RLF `(ramped ,hz ,(shape 1))) (df 'RLM `(ramped ,mix ,(shape 1))))))))

(def no-record-lp (v)
  (if (= v :static)
    (list '(defmacro rlg (fhz) 1) '(defmacro rlg-hit (fhz) 1))
    (list '(def rlm-d 0) '(def rlf-d 1) '(defmacro rlg-at (fhz rlm rlf) 1) '(defmacro rlg (fhz) 1))))

;; Timing and output gain: onset delay t0 (the whole voice starts t0 after the
;; hit), end of the hit (x length; x rhythm above 1), fade, gain. Required: the
;; assembler reads T0, EE / emax-v, FD and LV.
(def timing (id &key (names nil))
  (part :timing id (list (list "t0" 1 'lin -0.002 0.008) (list "end" 1 'lin 0.02 1.6) (list "fade" 1 'log 0.0002 0.2)
                         (list "gain" 1 'log 0.05 20))
        (dict :names names) timing-build))

(def timing-build (p ctx v)
  (if (= v :static)
    (dict :prelude (list (df 'T0 `(at-hit (max 0 ,(D p "t0")))))
          :hit (list (df 'EE `(at-hit (* length (max 1 rhythm) ,(D p "end"))))
                     (df 'FD `(at-hit ,(D p "fade")))
                     (df 'out-trim '(training-value 1 0.5))
                     (df 'LV `(at-hit (* out-trim level vgain ,(D p "gain"))))))
    (dict :prelude (list (df 'T0 `(at-hit (max 0 ,(D p "t0")))))
          :hit (list (df 'FD `(ramped ,(D p "fade") ,(shape 1)))
                     (df 'LV `(falling (* out-trim level vgain ,(D p "gain")) ,(shape 1)))
                     ;; end gate time: the LATEST end decoded so far this hit, so a morph never cuts a hit short
                     (df 'Enow `(latch (* length (max 1 rhythm) ,(D p "end")) dclk))
                     '(make-tensor-history emax @shape [1 1])
                     '(def emax-v (gswitch hit Enow (max (read-tensor-history emax) Enow)))
                     '(write-tensor-history emax emax-v)))))

;; ── noise bands ───────────────────────────────────────────────────────────
;; 16 fixed bands (2 cascaded RBJ band-pass sections each) on one white noise,
;; with a two-stage envelope (fast snap, slow buzz) after an onset delay and a
;; rise: snare wires. Other parts (hiss, events, wash) reuse the bank's `bands`.
(def band-fc-values '(48.23634 70.14603 102.0074 148.3408 215.7195 313.7026 456.1912 663.4002 964.7268 1402.921
                      2040.149 2966.816 4314.39 6274.052 9123.823 13268))

;; :envelope false: only the bank (for rattles, washes, events), no wires of its own.
;; The bank's centre frequencies (every bands part defines them; one bank per voice).
(def band-fc-form () `(def band-fc (tensor @shape [1 16] @data ,(rng (str "[" (apply str (map (lambda (x) (str x " ")) band-fc-values)) "]")))))

(def bands (id &key (envelope true) (names nil))
  (part :bands id (if envelope
                    (list (list "wl" 16 'log 0.0000001 4) (list "wm" 16 'lin 0 1) (list "wd1" 16 'log 0.0002 0.1)
                          (list "wd2" 16 'log 0.002 2) (list "w_t0" 1 'lin 0 0.02) (list "wrise" 1 'log 0.00001 0.05))
                    (list))
        (dict :envelope envelope :names names) bands-build))

(def bank-only (v)
  (dict :prelude (list (band-fc-form))
        :dsp (bank-forms)))

(def bank-forms ()
  (list '(def band-q (tensor @shape [1 16] @data [2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926 2.654926]))
        '(def bw0 (/ (* twopi (min band-fc (* 0.49 samplerate))) samplerate))
        '(def balpha (/ (sin bw0) (* 2 band-q)))
        '(def bb0 (/ balpha (+ 1 balpha)))
        '(def ba1 (/ (* -2 (cos bw0)) (+ 1 balpha)))
        '(def ba2 (/ (- 1 balpha) (+ 1 balpha)))
        '(def bnorm (gswitch (lt samplerate 46050) (tensor @shape [1 16] @data [39.30977 32.59781 27.03205 22.41686 18.59012 15.4175 12.78783 10.60932 8.806535 7.318115 6.095248 5.101119 4.311929 3.721605 3.359741 3.383705])
                      (gswitch (lt samplerate 68100) (tensor @shape [1 16] @data [41.01111 34.00862 28.20193 23.38693 19.39444 16.08428 13.34042 11.06696 9.184993 7.630139 6.350831 5.307525 4.473257 3.837144 3.417445 3.322431])
                        (gswitch (lt samplerate 92100) (tensor @shape [1 16] @data [55.59221 46.09992 38.22848 31.70116 26.28853 21.80035 18.07896 14.99376 12.43669 10.31857 8.566192 7.120135 5.933383 4.970944 4.211214 3.651693])
                          (tensor @shape [1 16] @data [57.99829 48.09515 39.88301 33.07316 27.42622 22.74371 18.86112 15.64216 12.974 10.76349 8.934013 7.423182 6.181248 5.170463 4.365924 3.759839])))))
        '(def white (* 3.4641016 (- (noise) 0.5)))
        '(make-history wx1)
        '(make-history wx2)
        '(def white-x2 (read-history wx2))
        '(write-history wx2 (read-history wx1))
        '(write-history wx1 white)
        '(make-tensor-history s1y1 @shape [1 16])
        '(make-tensor-history s1y2 @shape [1 16])
        '(make-tensor-history s2y1 @shape [1 16])
        '(make-tensor-history s2y2 @shape [1 16])
        '(def s1-1 (read-tensor-history s1y1))
        '(def s1-2 (read-tensor-history s1y2))
        '(def s2-1 (read-tensor-history s2y1))
        '(def s2-2 (read-tensor-history s2y2))
        '(def stage1 (- (* bb0 (- white white-x2)) (+ (* ba1 s1-1) (* ba2 s1-2))))
        '(def stage2 (- (* bb0 (- stage1 s1-2)) (+ (* ba1 s2-1) (* ba2 s2-2))))
        '(write-tensor-history s1y2 s1-1)
        '(write-tensor-history s1y1 stage1)
        '(write-tensor-history s2y2 s2-1)
        '(write-tensor-history s2y1 stage2)
        '(def bands (* bnorm stage2))))

(def bands-build (p ctx v)
  (if (= (get p :envelope) false) (bank-only v)
  (let ((iW1 (s p "iW1")) (iW2 (s p "iW2")) (WT0 (s p "WT0")) (iWR (s p "iWR")) (WGF (s p "WGF")) (WGS (s p "WGS"))
        (TW (s p "tw")) (RISE (s p "rise-env")) (WENV (s p "wenv")) (OUT (s p "out"))
        (att (filter (lambda (q) (and (get q :attach) (= (get q :on) (get p :id)))) (get ctx :parts))))
    (let ((attached (map (lambda (q) ((get q :attach) q ctx v)) att)))
     (if (= v :static)
      (dict :prelude (list (band-fc-form))
            :iwr iWR
            :hit (list (df iW1 `(at-hit (/ 1 (* wire_decay ,(D p "wd1")))))
                       (df iW2 `(at-hit (/ 1 (* wire_decay ,(D p "wd2")))))
                       (df WT0 `(at-hit ,(D p "w_t0")))
                       (df iWR `(at-hit (/ 1 (* attack ,(D p "wrise")))))
                       (df WGF `(at-hit (* snappy vsnap ,(D p "wl") ,(D p "wm") (rlg-hit band-fc))))
                       (df WGS `(at-hit (* snappy ,(D p "wl") (- 1 ,(D p "wm")) (rlg-hit band-fc)))))
            :dsp (concat-all (list
                   (bank-forms)
                   (list (df TW `(max 0 (- t ,WT0)))
                         (df RISE `(- 1 (exp (* -1 ,TW ,iWR))))
                         (df WENV `(+ (* ,WGF (exp (* -1 ,TW ,iW1))) (* ,WGS (exp (* -1 ,TW ,iW2))))))
                   (concat-all (map (lambda (a) ((get a :dsp) iWR)) attached))
                   (list (df OUT `(sum-axis (* ,(plus (cons `(* ,RISE ,WENV) (map (lambda (a) (get a :term)) attached))) bands)
                                            @axis 1 @keepdims true)))))
            :drum (list OUT))
      (dict :prelude (list (band-fc-form))
            :hit (list (df iW1 `(ramped (/ 1 (* wire_decay ,(D p "wd1"))) ,(shape 16)))
                       (df iW2 `(ramped (/ 1 (* wire_decay ,(D p "wd2"))) ,(shape 16)))
                       (df WGF `(falling (* snappy vsnap ,(D p "wl") ,(D p "wm") (rlg-at band-fc rlm-d rlf-d)) ,(shape 16)))
                       (df WGS `(falling (* snappy ,(D p "wl") (- 1 ,(D p "wm")) (rlg-at band-fc rlm-d rlf-d)) ,(shape 16))))
            :fixed (list (df WT0 `(at-hit ,(D p "w_t0")))
                         (df iWR `(at-hit (/ 1 (* attack ,(D p "wrise"))))))
            :dsp (concat-all (list
                   (bank-forms)
                   (list (df TW `(max 0 (- t ,WT0)))
                         (df (s p "n1") `(decaying ,iW1 (+ T0 ,WT0) ,(shape 16)))
                         (df (s p "n2") `(decaying ,iW2 (+ T0 ,WT0) ,(shape 16)))
                         (df WENV `(* (- 1 (exp (* -1 ,TW ,iWR))) (+ (* ,WGF ,(s p "n1")) (* ,WGS ,(s p "n2"))))))
                   (concat-all (map (lambda (a) ((get a :dsp) iWR)) attached))
                   (list (df OUT `(sum-axis (* ,(plus (cons WENV (map (lambda (a) (get a :term)) attached))) bands)
                                            @axis 1 @keepdims true)))))
            :drum (list OUT)))))))

;; Hiss: stationary record noise on a bank's bands (one slow decay), x noise x
;; record, on the bank's rise. Summed inside the bank's band sum.
(def hiss (id &key (on 'wires) (names nil))
  (part :hiss id (list (list "hl" 16 'log 0.0000001 1) (list "hdec" 1 'log 0.1 30))
        (dict :on on :names names :attach hiss-attach) hiss-build))

(def hiss-build (p ctx v)
  (if (= v :static)
    (dict :hit (list (df (s p "iHD") `(at-hit (/ 1 ,(D p "hdec"))))
                     (df (s p "HG") `(at-hit (* noise record ,(D p "hl") (rlg-hit band-fc))))))
    (dict :hit (list (df (s p "HG") `(falling (* noise record ,(D p "hl") (rlg-at band-fc rlm-d rlf-d)) ,(shape 16))))
          :fixed (list (df (s p "iHD") `(at-hit (/ 1 ,(D p "hdec"))))))))

(def hiss-attach (p ctx v)
  (let ((iHD (s p "iHD")) (HG (s p "HG")) (ENV (s p "env")))
    (dict :dsp (lambda (iWR)
                 (if (= v :static)
                   (list (df ENV `(* (exp (* -1 t ,iHD)) (- 1 (exp (* -1 t ,iWR))))))
                   (list (df ENV `(* ,HG (exp (* -1 t ,iHD)) (- 1 (exp (* -1 t ,iWR))))))))
          :term (if (= v :static) `(* ,ENV ,HG) ENV))))

;; Crackle: Bernoulli(rate/sr) impulses with heavy-tailed amplitude and random
;; sign through 4 bands, x record; after the clipper.
(def crackle (id &key (names nil))
  (part :crackle id (list (list "cr_lvl" 1 'log 0.000001 4) (list "cr_rate" 1 'log 1 4000) (list "cr_col" 4 'log 0.001 10)
                          (list "cr_dec" 1 'log 0.05 20))
        (dict :names names) crackle-build))

(def crackle-build (p ctx v)
  (let ((CRL (s p "CRL")) (CRP (s p "CRP")) (CRC (s p "CRC")) (iCRD (s p "iCRD")) (OUT (s p "out")))
    (let ((chain (list '(def cr-n (* 0.5 (+ 1 (noise @size 3))))
                       '(def cr-u (sum (shrink cr-n @ranges [0:1])))
                       '(def cr-a (sum (shrink cr-n @ranges [1:2])))
                       '(def cr-s (sum (shrink cr-n @ranges [2:3])))
                       `(def cr-imp (* (lt cr-u ,CRP) (min 30 (/ 1 (sqrt (max cr-a 0.001)))) (- (* 2 (gt cr-s 0.5)) 1)))
                       '(def cr-fc (tensor @shape [1 4] @data [1000 2500 6000 12000]))
                       '(def cw0 (/ (* twopi (min cr-fc (* 0.49 samplerate))) samplerate))
                       '(def calpha (/ (sin cw0) 1.6))
                       '(def cb0 (/ calpha (+ 1 calpha)))
                       '(def ca1 (/ (* -2 (cos cw0)) (+ 1 calpha)))
                       '(def ca2 (/ (- 1 calpha) (+ 1 calpha)))
                       '(make-tensor-history cix1 @shape [rows 1])
                       '(make-tensor-history cix2 @shape [rows 1])
                       '(def cimp2 (read-tensor-history cix2))
                       '(write-tensor-history cix2 (read-tensor-history cix1))
                       '(write-tensor-history cix1 cr-imp)
                       '(make-tensor-history ciy1 @shape [rows 4])
                       '(make-tensor-history ciy2 @shape [rows 4])
                       '(def cy1 (read-tensor-history ciy1))
                       '(def cy2 (read-tensor-history ciy2))
                       '(def cband (- (* cb0 (- cr-imp cimp2)) (+ (* ca1 cy1) (* ca2 cy2))))
                       '(write-tensor-history ciy2 cy1)
                       '(write-tensor-history ciy1 cband))))
      (if (= v :static)
        (dict :hit (list (df CRL `(at-hit (* record ,(D p "cr_lvl"))))
                         (df CRP `(at-hit (/ ,(D p "cr_rate") samplerate)))
                         (df CRC `(at-hit ,(D p "cr_col")))
                         (df iCRD `(at-hit (/ 1 ,(D p "cr_dec")))))
              :dsp (append chain (list (df OUT `(* ,CRL (exp (* -1 t ,iCRD)) (sum-axis (* ,CRC cband) @axis 1 @keepdims true)))))
              :post (list OUT))
        (dict :hit (list (df CRC `(ramped ,(D p "cr_col") ,(shape 4)))
                         (df iCRD `(ramped (/ 1 ,(D p "cr_dec")) ,(shape 1)))
                         (df CRL `(falling (* record ,(D p "cr_lvl")) ,(shape 1))))
              :fixed (list (df CRP `(at-hit (/ ,(D p "cr_rate") samplerate))))
              :dsp (append chain (list (df (s p "crd") `(decaying ,iCRD T0 ,(shape 1)))
                                       (df OUT `(* ,CRL ,(s p "crd") (sum-axis (* ,CRC cband) @axis 1 @keepdims true)))))
              :post (list OUT))))))

;; Two noise events on a bank's bands, each with its own time (x length, x rhythm
;; past 50 ms), level (x record), decay, rise and band colour: second hits,
;; ghost notes, a hat after the snare.
(def events (id &key (on 'wires) (names nil))
  (part :events id (list (list "ev_t" 2 'lin 0 1.2) (list "ev_lvl" 2 'log 0.000001 4) (list "ev_dec" 2 'log 0.002 1)
                         (list "ev_rise" 2 'log 0.0001 0.02) (list "ev_col" 32 'log 0.0001 1))
        (dict :on on :names names) events-build))

;; rhythm: events in the first 25 ms (attack texture) stay put; from 50 ms on a
;; time is multiplied by rhythm; a smoothstep joins the two.
(def rhythm-forms (T0name wname Tname)
  (list (df wname `(clip (/ (- ,T0name 0.025) 0.025) 0 1))
        (df Tname `(at-hit (* ,T0name (+ 1 (* (* ,wname ,wname (- 3 (* 2 ,wname))) (- rhythm 1))))))))

(def events-build (p ctx v)
  (let ((EVT0 (s p "EVT0")) (EVT (s p "EVT")) (EVL (s p "EVL")) (iEVD (s p "iEVD")) (iEVR (s p "iEVR"))
        (C1 (s p "C1")) (C2 (s p "C2")) (ET (s p "evt")) (ENV (s p "env")) (O1 (s p "out1")) (O2 (s p "out2")))
    (let ((hit (append (list (df EVT0 `(at-hit (* length ,(D p "ev_t")))))
                       (rhythm-forms EVT0 (s p "w") EVT)
                       (list (df EVL `(at-hit (* record ,(D p "ev_lvl"))))
                             (df iEVD `(at-hit (/ 1 (* decay ,(D p "ev_dec")))))
                             (df iEVR `(at-hit (/ 1 ,(D p "ev_rise"))))
                             (df C1 `(at-hit (shrink ,(D p "ev_col") @ranges ,(rng "[:,0:16]"))))
                             (df C2 `(at-hit (shrink ,(D p "ev_col") @ranges ,(rng "[:,16:32]"))))))))
      (dict (if (= v :static) :hit :fixed) hit
            :dsp (list (df ET `(max 0 (- t ,EVT)))
                       (df ENV `(* ,EVL (gt t ,EVT) (- 1 (exp (* -1 ,ET ,iEVR))) (exp (* -1 ,ET ,iEVD))))
                       (df O1 `(* (shrink ,ENV @ranges ,(rng "[:,0:1]")) (sum-axis (* ,C1 bands) @axis 1 @keepdims true)))
                       (df O2 `(* (shrink ,ENV @ranges ,(rng "[:,1:2]")) (sum-axis (* ,C2 bands) @axis 1 @keepdims true))))
            :drum (list O1 O2)))))

;; ── template: a fixed mode table warped by a few learned numbers ──────────
;; :freqs :taus :mags :phases (equal-length lists; a measured or solved object's
;; modes). Learned: size (frequency scale), level, tilt ((f/3k)^tilt), decay
;; scale. Not note-tracked; x knob; :strokes re-excited by the second stroke.
;; :keep N keeps the N modes with the most energy (mag^2 tau) — CPU scales with N.
;; :second :absolute gives the second stroke its own level (lvl2) instead of the
;; restrike's level x the first stroke's: a ride hit a 16th after a snare whose
;; first-stroke ride is near silent (break-snares B28).
(def template (id &key (freqs nil) (taus nil) (mags nil) (phases nil) (modes nil) (knob nil) (strokes false)
                       (keep nil) (second :relative) (names nil))
  (if modes
    (template id :freqs (get modes :freqs) :taus (get modes :taus) :mags (get modes :mags) :phases (get modes :phases)
              :knob knob :strokes strokes :keep keep :second second :names names)
  (let ((idx (strongest (map (lambda (k) (* (nth mags k) (nth mags k) (nth taus k))) (range (len mags)))
                        (if keep (min keep (len mags)) (len mags)))))
    (let ((pick (lambda (xs) (map (lambda (k) (nth xs k)) idx))))
      (part :template id
            (append (list (list "size" 1 'log 0.6 1.6) (list "lvl" 1 'log 0.0000001 4) (list "tilt" 1 'lin -2 2)
                          (list "dec" 1 'log 0.1 10))
                    (if (and strokes (= second :absolute)) (list (list "lvl2" 1 'log 0.0000001 4)) (list)))
            (dict :freqs (pick freqs) :taus (pick taus) :mags (pick mags) :phases (pick phases) :knob knob
                  :strokes strokes :second second :names names)
            template-build)))))

;; A template's mode table from an eseq.modal data object (m/solid, m/modal-data,
;; m/net, m/web: anything with :freqs, :losses and :shapes), heard at one of its
;; points (an index into its shapes): frequency, decay (1 / loss), magnitude
;; (|shape| at the point, unit norm) and a spread of starting phases. Modes above
;; fmax are dropped. (t/template 'bell :modes (t/modes-of (m/solid 'b ...) :point 3) :keep 24)
(def modes-of (obj &key (point 0) (fmax 16000))
  (let ((fs (get obj :freqs)) (ls (get obj :losses)) (shapes (get obj :shapes)))
    (let ((sh (nth shapes (min point (- (len shapes) 1))))
          (ks (filter (lambda (k) (< (nth fs k) fmax)) (range (len fs)))))
      (let ((mag (map (lambda (k) (abs (nth sh k))) ks)))
        (let ((norm (sqrt (max (reduce (lambda (acc m) (+ acc (* m m))) 0 mag) 0.000000000001))))
          (dict :freqs (map (lambda (k) (nth fs k)) ks)
                :taus (map (lambda (k) (/ 1 (max (nth ls k) 0.01))) ks)
                :mags (map (lambda (m) (/ m norm)) mag)
                ;; golden-angle phases: spread, deterministic
                :phases (map (lambda (j) (- (* 2.39996323 j) (* 6.28318531 (floor (/ (* 2.39996323 j) 6.28318531))) 3.14159265))
                             (range (len ks)))))))))

;; Indices of the n largest values of xs, in their original order when n = len.
(def strongest (xs n)
  (if (= n (len xs)) (range n)
    (reduce (lambda (chosen j)
              (let ((rest-idx (filter (lambda (k) (not (member? chosen k))) (range (len xs)))))
                (append chosen (list (reduce (lambda (best k) (if (> (nth xs k) (nth xs best)) k best))
                                             (first rest-idx) rest-idx)))))
            (list) (range n))))

(def tensor-row (name xs)
  (df name `(tensor @shape ,(shape (len xs)) @data ,(rng (str "[" (apply str (map (lambda (k) (str (if (= k 0) "" " ") (nth xs k))) (range (len xs)))) "]")))))

(def template-build (p ctx v)
  (let ((n (len (get p :freqs))) (st (if (get p :strokes) (stroke-of ctx) nil)) (abs2 (= (get p :second) :absolute))
        (knobs (if (get p :knob) (list (get p :knob)) (list)))
        (TF (s p "F0")) (TT (s p "TAU")) (TM (s p "M")) (TP (s p "PH0")) (A2 (s p "A2"))
        (RBF (s p "F")) (RBA (s p "A")) (iRBD (s p "iD")) (RF (s p "f")) (ENV (s p "env")) (OUT (s p "out")))
    (let ((pre (list (tensor-row TF (get p :freqs)) (tensor-row TT (get p :taus))
                     (tensor-row TM (get p :mags)) (tensor-row TP (get p :phases))))
          (amp (lambda (lv f rl) `(* ,@knobs ,(D p lv) ,TM (exp (* ,(D p "tilt") (log (/ ,f 3000)))) ,(alias-form f) ,rl))))
     (let ((second-static (if st (if abs2 `(* ,(s st "on2a") ,A2 (exp (* -1 ,(s st "t2") ,iRBD)))
                                   `(* ,RBA ,(s st "on2") (exp (* -1 ,(s st "t2") ,iRBD))))
                            nil)))
      (if (= v :static)
        (dict :prelude pre
              :hit (append (list (df RF `(* ,(D p "size") ,TF))
                                 (df RBF `(at-hit ,RF))
                                 (df RBA `(at-hit ,(amp "lvl" RF `(rlg-hit ,RF))))
                                 (df iRBD `(at-hit (/ 1 (* decay ,(D p "dec") ,TT)))))
                           (if (and st abs2) (list (df A2 `(at-hit ,(amp "lvl2" RF `(rlg-hit ,RF))))) (list)))
              :dsp (list (df ENV (if st
                                   (if abs2 `(+ (* ,RBA (exp (* -1 t ,iRBD)) (- 1 (exp (* -10000 t)))) ,second-static)
                                     `(* ,RBA (+ (* (exp (* -1 t ,iRBD)) (- 1 (exp (* -10000 t)))) (* ,(s st "on2") (exp (* -1 ,(s st "t2") ,iRBD))))))
                                   `(* ,RBA (exp (* -1 t ,iRBD)) (- 1 (exp (* -10000 t))))))
                         (df OUT `(sum-axis (* ,ENV (sin (+ (* twopi ,RBF t) ,TP))) @axis 1 @keepdims true)))
              :drum (list OUT))
        (dict :prelude pre
              :hit (append (list (df RF `(* ,(D p "size") ,TF))
                                 (df RBA `(falling ,(amp "lvl" RF `(rlg-at ,RF rlm-d rlf-d)) ,(shape n)))
                                 (df iRBD `(ramped (/ 1 (* decay ,(D p "dec") ,TT)) ,(shape n))))
                           (if (and st abs2) (list (df A2 `(falling ,(amp "lvl2" RF `(rlg-at ,RF rlm-d rlf-d)) ,(shape n)))) (list)))
              :fixed (list (df RBF `(at-hit (* ,(D p "size") ,TF))))
              :dsp (append (list (df (s p "dn") `(decaying ,iRBD T0 ,(shape n))))
                           (if st (list (df (s p "dn2") `(decaying ,iRBD (+ T0 ,(s st "MRT")) ,(shape n)))) (list))
                           (list (df OUT `(sum-axis (* ,@(if st
                                                           (if abs2 (list `(+ (* ,RBA ,(s p "dn") (- 1 (exp (* -10000 t)))) (* ,(s st "on2a") ,A2 ,(s p "dn2"))))
                                                             (list RBA `(+ (* ,(s p "dn") (- 1 (exp (* -10000 t)))) (* ,(s st "on2") ,(s p "dn2")))))
                                                           (list RBA `(* ,(s p "dn") (- 1 (exp (* -10000 t))))))
                                                      (sin (+ (* twopi ,RBF t) ,TP))) @axis 1 @keepdims true))))
              :drum (list OUT)))))))

;; ── wash: a bank's bands again, swelling after the stroke ─────────────────
;; :second :absolute: the second stroke's wash has its own band gains (l2).
(def wash (id &key (on 'wires) (knob nil) (strokes false) (second :relative) (names nil))
  (part :wash id (append (list (list "l" 16 'log 0.0000001 4) (list "rise" 1 'log 0.0005 0.2) (list "dec" 1 'log 0.02 6))
                         (if (and strokes (= second :absolute)) (list (list "l2" 16 'log 0.0000001 4)) (list)))
        (dict :on on :knob knob :strokes strokes :second second :names names) wash-build))

(def wash-build (p ctx v)
  (let ((st (if (get p :strokes) (stroke-of ctx) nil)) (abs2 (= (get p :second) :absolute))
        (knobs (if (get p :knob) (list (get p :knob)) (list)))
        (G (s p "G")) (G2 (s p "G2")) (iR (s p "iR")) (iD (s p "iD")) (W1 (s p "w1")) (W2 (s p "w2")) (OUT (s p "out")))
    (let ((gain2 (if (and st abs2) (list (df G2 (if (= v :static) `(at-hit (* ,@knobs ,(D p "l2") (rlg-hit band-fc)))
                                                  `(falling (* ,@knobs ,(D p "l2") (rlg-at band-fc rlm-d rlf-d)) ,(shape 16)))))
                   (list)))
          (on (if st (if abs2 (s st "on2a") (s st "MRL")) nil))
          (mix (if st (if abs2 `(+ (* ,W1 ,G) (* ,W2 ,G2)) `(* (+ ,W1 ,W2) ,G)) `(* ,W1 ,G))))
     (if (= v :static)
      (dict :hit (append (list (df G `(at-hit (* ,@knobs ,(D p "l") (rlg-hit band-fc))))
                               (df iR `(at-hit (/ 1 ,(D p "rise"))))
                               (df iD `(at-hit (/ 1 (* decay ,(D p "dec"))))))
                         gain2)
            :dsp (append (list (df W1 `(* (- 1 (exp (* -1 t ,iR))) (exp (* -1 t ,iD)))))
                         (if st (list (df W2 (if abs2 `(* ,on (- 1 (exp (* -1 ,(s st "t2") ,iR))) (exp (* -1 ,(s st "t2") ,iD)))
                                               `(* ,on (gt t ,(s st "MRT")) (- 1 (exp (* -1 ,(s st "t2") ,iR))) (exp (* -1 ,(s st "t2") ,iD))))))
                           (list))
                         (list (df OUT (if (and st (not abs2)) `(sum-axis (* (+ ,W1 ,W2) ,G bands) @axis 1 @keepdims true)
                                         `(sum-axis (* ,mix bands) @axis 1 @keepdims true)))))
            :drum (list OUT))
      (dict :hit (append (list (df G `(falling (* ,@knobs ,(D p "l") (rlg-at band-fc rlm-d rlf-d)) ,(shape 16)))
                               (df iD `(ramped (/ 1 (* decay ,(D p "dec"))) ,(shape 1))))
                         gain2)
            :fixed (list (df iR `(at-hit (/ 1 ,(D p "rise")))))
            :dsp (append (list (df (s p "d1") `(decaying ,iD T0 ,(shape 1))))
                         (if st (list (df (s p "d2") `(decaying ,iD (+ T0 ,(s st "MRT")) ,(shape 1)))) (list))
                         (list (df W1 `(* (- 1 (exp (* -1 t ,iR))) ,(s p "d1"))))
                         (if st (list (df W2 (if abs2 `(* ,on (- 1 (exp (* -1 ,(s st "t2") ,iR))) ,(s p "d2"))
                                               `(* ,on (gt t ,(s st "MRT")) (- 1 (exp (* -1 ,(s st "t2") ,iR))) ,(s p "d2")))))
                           (list))
                         (list (df OUT (if (and st (not abs2)) `(sum-axis (* (+ ,W1 ,W2) ,G bands) @axis 1 @keepdims true)
                                         `(sum-axis (* ,mix bands) @axis 1 @keepdims true)))))
            :drum (list OUT))))))

;; ── a second stroke ───────────────────────────────────────────────────────
;; One decoded time (x length, x rhythm past 50 ms) and level, relative to the
;; first stroke. Parts made with :strokes true read its t2 / on2 / MRT / MRL.
(def restrike (id &key (names nil))
  (part :restrike id (list (list "t" 1 'lin 0.02 0.8) (list "lvl" 1 'log 0.0001 4)) (dict :names names) restrike-build))

(def restrike-build (p ctx v)
  (let ((T0n (s p "MRT0")) (MRT (s p "MRT")) (MRL (s p "MRL")) (T2 (s p "t2")) (ON2 (s p "on2")))
    (let ((hit (append (list (df T0n `(at-hit (* length ,(D p "t")))))
                       (rhythm-forms T0n (s p "w") MRT)
                       (list (df MRL `(at-hit ,(D p "lvl")))))))
      (dict (if (= v :static) :hit :fixed) hit
            :stroke (append (list (df T2 `(max 0 (- t ,MRT)))
                                  (df ON2 `(* ,MRL (gt t ,MRT) (- 1 (exp (* -10000 ,T2))))))
                            ;; an absolute second stroke (a part's own lvl2) starts without the relative level
                            (if (any? (lambda (q) (= (get q :second) :absolute)) (get ctx :parts))
                              (list (df (s p "on2a") `(* (gt t ,MRT) (- 1 (exp (* -10000 ,T2))))))
                              (list)))))))

;; ── strings: note-tracked partials, optionally prepared ───────────────────
;; Partial k (1..n) at k f0 sqrt(1 + B k^2) (stiff-string inharmonicity B), its own
;; level, decay and phase, an instant rise. Treatments are a few learned numbers
;; that act on every partial the way the object would (feed-forward: they shape
;; the partials, nothing feeds back):
;;   :bolt  a mass m at position x along the string: partials moving there drop
;;          in pitch by 1/sqrt(1 + m sin^2(pi k x)) (a screw, a bolt, a coin)
;;   :mute  a damper of depth d at position x: partials moving there decay
;;          (1 + d sin^2(pi k x)) times faster (rubber, felt, a finger)
;; f0 is the played note x tune. :knob scales the level.
(def strings (id &key (partials 24) (bolt false) (mute false) (strokes false) (bus :drum) (knob nil) (names nil))
  (part :strings id
        (concat-all (list (list (list "b" 1 'log 0.000001 0.01)
                                (list "amp" partials 'log 0.0000001 4)
                                (list "dec" partials 'log 0.005 20)
                                (list "ph" partials 'raw))
                          (if bolt (list (list "bx" 1 'lin 0.01 0.5) (list "bm" 1 'log 0.0001 4)) (list))
                          (if mute (list (list "mx" 1 'lin 0.01 0.5) (list "md" 1 'log 0.01 100)) (list))))
        (dict :count partials :bolt bolt :mute mute :strokes strokes :bus bus :knob knob :names names :follow-note true)
        strings-build))

(def strings-build (p ctx v)
  (let ((n (get p :count)) (st (if (get p :strokes) (stroke-of ctx) nil)) (knobs (if (get p :knob) (list (get p :knob)) (list)))
        (K (s p "k")) (F (s p "F")) (A (s p "A")) (iD (s p "iD")) (PH (s p "PH")) (FHZ (s p "fhz"))
        (ENV (s p "env")) (OUT (s p "out")))
    (let ((kt (list (df K `(tensor @shape ,(shape n) @data ,(rng (str "[" (apply str (map (lambda (k) (str (if (= k 0) "" " ") (+ k 1))) (range n))) "]"))))))
          ;; the treatments' factors per partial (from the decoded row, so they can be held)
          (bolt (if (get p :bolt) `(/ 1 (sqrt (+ 1 (* ,(D p "bm") (* (sin (* 3.14159265 ,K ,(D p "bx"))) (sin (* 3.14159265 ,K ,(D p "bx")))))))) 1))
          (mute (if (get p :mute) `(+ 1 (* ,(D p "md") (* (sin (* 3.14159265 ,K ,(D p "mx"))) (sin (* 3.14159265 ,K ,(D p "mx")))))) 1))
          (outs (if (= (get p :bus) :body) (dict :body (list OUT) :ms (list ENV)) (dict :drum (list OUT)))))
      (let ((fhz `(* pitch tune ,K (sqrt (+ 1 (* ,(D p "b") ,K ,K))) ,bolt)))
       (if (= v :static)
        (merged outs (dict
          :prelude kt
          :hit (list (df FHZ fhz)
                     (df F `(at-hit ,FHZ))
                     (df A `(at-hit (* ,@knobs ,(D p "amp") ,(alias-form FHZ) (rlg-hit ,FHZ))))
                     (df iD `(at-hit (/ ,mute (* decay ,(D p "dec")))))
                     (df PH `(at-hit ,(D p "ph"))))
          :dsp (list (df ENV (if st `(* ,A (+ (* (exp (* -1 t ,iD)) (- 1 (exp (* -10000 t)))) (* ,(s st "on2") (exp (* -1 ,(s st "t2") ,iD)))))
                               `(* ,A (exp (* -1 t ,iD)) (- 1 (exp (* -10000 t))))))
                     (df OUT `(sum-axis (* ,ENV (sin (+ (* twopi ,F t) ,PH))) @axis 1 @keepdims true)))))
        (merged outs (dict
          :prelude kt
          :hit (list (df FHZ fhz)
                     (df A `(falling (* ,@knobs ,(D p "amp") ,(alias-form FHZ) (rlg-at ,FHZ rlm-d rlf-d)) ,(shape n)))
                     (df iD `(ramped (/ ,mute (* decay ,(D p "dec"))) ,(shape n))))
          :fixed (list (df F `(at-hit ,FHZ)) (df PH `(at-hit ,(D p "ph"))))
          :dsp (append (list (df (s p "dn") `(decaying ,iD T0 ,(shape n))))
                       (if st (list (df (s p "dn2") `(decaying ,iD (+ T0 ,(s st "MRT")) ,(shape n)))) (list))
                       (list (df ENV (if st `(* ,A (+ (* ,(s p "dn") (- 1 (exp (* -10000 t)))) (* ,(s st "on2") ,(s p "dn2"))))
                                       `(* ,A ,(s p "dn") (- 1 (exp (* -10000 t))))))
                             (df OUT `(sum-axis (* ,ENV (sin (+ (* twopi ,F t) ,PH))) @axis 1 @keepdims true)))))))))))

;; ── rattle: a bank's bands gated by another part's loudness ───────────────
;; A loose screw, a paperclip, a sitar's bridge, snare wires buzzing in
;; sympathy: noise in learned band gains, on while the source part's summed
;; envelope is above a learned threshold (feed-forward: it reads the source,
;; never pushes back). :of a part with an envelope (modes, strings, template).
(def rattle (id &key (on 'buzz) (of nil) (names nil))
  (part :rattle id (list (list "l" 16 'log 0.0000001 4) (list "thr" 1 'log 0.00001 1) (list "soft" 1 'log 0.0001 1))
        (dict :on on :of of :names names) rattle-build))

(def rattle-build (p ctx v)
  (let ((G (s p "G")) (THR (s p "THR")) (SOFT (s p "SOFT")) (LOUD (s p "loud")) (GATE (s p "gate")) (OUT (s p "out"))
        (src (s (dict :id (get p :of)) "env")))
    (dict :hit (if (= v :static)
                 (list (df G `(at-hit (* ,(D p "l") (rlg-hit band-fc)))) (df THR `(at-hit ,(D p "thr"))) (df SOFT `(at-hit ,(D p "soft"))))
                 (list (df G `(falling (* ,(D p "l") (rlg-at band-fc rlm-d rlf-d)) ,(shape 16)))
                       (df THR `(ramped ,(D p "thr") ,(shape 1))) (df SOFT `(ramped ,(D p "soft") ,(shape 1)))))
          :dsp (list (df LOUD `(sum-axis (abs ,src) @axis 1 @keepdims true))
                     ;; a soft knee: 0 below the threshold, rising over `soft`
                     (df GATE `(clip (/ (- ,LOUD ,THR) ,SOFT) 0 1))
                     (df OUT `(* ,GATE (sum-axis (* ,G bands) @axis 1 @keepdims true))))
          :drum (list OUT))))

;; ── variants: a voice made from a parent voice ────────────────────────────
;; (t/variant parent "name" :add (list parts) :swap (list parts) :remove '(ids))
;; :swap replaces the parts with the same ids in place; :add appends parts before
;; the second stroke (or at the end). :latent changes the decoder's code size. The lineage (parent, added, swapped,
;; removed) goes into layout.json: the runner seeds added and swapped parts
;; afresh (the parent's rows for them do not apply), keeps the parent's rows for
;; the rest, and checks that with them silent the voice still renders the parent.
(def variant (v name &key (add nil) (swap nil) (remove nil) (latent nil))
  (let ((ids-of (lambda (ps) (map (lambda (q) (get q :id)) ps))))
    (let ((kept (filter (lambda (q) (not (member? remove (get q :id)))) (get v :parts)))
          (swapped (ids-of swap)))
      (let ((replaced (map (lambda (q) (let ((hit (filter (lambda (r) (= (get r :id) (get q :id))) swap)))
                                         (if (empty? hit) q (first hit))))
                           kept)))
        (let ((strokes (filter (lambda (q) (= (get q :kind) :restrike)) replaced))
              (others (filter (lambda (q) (not (= (get q :kind) :restrike))) replaced)))
          (merged v (dict :name name :latent (if latent latent (get v :latent))
                          :parts (append others add strokes)
                          :lineage (dict :parent (get v :name) :added (map source (ids-of add))
                                         :swapped (map source swapped) :removed (map source (if remove remove (list)))))))))))

;; ── knobs (identity at their defaults); only the ones the voice reads ─────
(def knob-table
  '((tune 1 0.5 2) (decay 1 0.25 4) (bend 1 0 3) (attack 1 0.25 4) (punch 1 0 3) (snappy 1 0 3)
    (wire_decay 1 0.25 4) (shape 1 0 3) (drive 1 0.25 8) (record 1 0 2) (noise 1 0 4) (length 1 0.25 4)
    (rhythm 1 0.25 4) (metal 1 0 4) (level 1 0 2) (vel_amt 1 0 2)))

(def knob-forms (forms)
  (let ((atoms (form-atoms forms)))
    (map (lambda (k) `(param ,(first k) @default ,(nth k 1) @min ,(nth k 2) @max ,(nth k 3)))
         (filter (lambda (k) (member? atoms (first k))) knob-table))))

;; ── the voice ─────────────────────────────────────────────────────────────

;; A trainable voice. :parts in decoder-layout order. :layout the row-layout's
;; name. :latent the code size. :hidden the decoder's hidden width.
(def voice (name &key (parts nil) (layout 'snare) (latent 4) (hidden 128) (doc nil))
  (dict :kind :train-voice :name name :parts parts :layout layout :latent latent :hidden hidden :doc doc))

(def segment-count (v)
  (reduce (lambda (acc p) (+ acc (reduce (lambda (a f) (+ a (nth f 1))) 0 (get p :fields)))) 0 (get v :parts)))

(def errors (v)
  (let ((ps (get v :parts)))
    (concat-all (list
      (if (find-part (dict :parts ps) :timing nil) nil (list "a voice needs a (timing ...) part"))
      (if (empty? (filter (lambda (p) (= (get p :kind) :restrike)) ps)) nil
        (if (> (len (filter (lambda (p) (= (get p :kind) :restrike)) ps)) 1) (list "one restrike per voice for now") nil))
      ;; every bands part defines the same bank names (band-fc, bands, its filter state)
      (if (> (len (filter (lambda (p) (= (get p :kind) :bands)) ps)) 1) (list "one bands part per voice for now") nil)
      (concat-all (map (lambda (p) (if (and (= (get p :kind) :rattle) (not (get p :of)))
                                     (list (str (source (get p :id)) ": a rattle needs :of (the part whose loudness gates it)"))
                                     nil))
                       ps))
      (concat-all (map (lambda (p) (if (and (get p :on) (not (find-part (dict :parts ps) :bands (get p :on))))
                                     (list (str (source (get p :id)) ": no bands part " (source (get p :on))))
                                     nil))
                       ps))))))

;; Every part's build for variant v, with the bus lists in ctx (the shaper reads
;; the body bus and the mean-square envelopes of the parts before it).
(def builds (v variant)
  (let ((ps (get v :parts)))
    (let ((pre (map (lambda (p) (if (= (get p :kind) :shaper) nil ((get p :build) p (dict :parts ps) variant))) ps)))
      (let ((body (concat-all (map (lambda (b) (if b (get b :body) nil)) pre)))
            (ms (concat-all (map (lambda (b) (if b (get b :ms) nil)) pre))))
        (map (lambda (k) (if (= (get (nth ps k) :kind) :shaper)
                           ((get (nth ps k) :build) (nth ps k) (dict :parts ps :body body :ms ms) variant)
                           (nth pre k)))
             (range (len ps)))))))

(def section (bs key) (concat-all (map (lambda (b) (let ((x (get b key))) (if x x (list)))) bs)))

(def layout-form (v)
  `(row-layout ,(get v :layout) ,@(concat-all (map (lambda (p) (get p :segments)) (get v :parts)))))

(def inputs-forms (morph)
  (append (list '(def gate (in 1 @name gate)) '(def pitch (in 2 @name pitch))
                '(def velocity (in 3 @name velocity)) '(def trigger (in 4 @name trigger)))
          (if morph (list '(def clock (in 5 @name clock))
                          '(def mod1 (in 6 @name mod1 @modulator 1)) '(def mod2 (in 7 @name mod2 @modulator 2))
                          '(def mod3 (in 8 @name mod3 @modulator 3)) '(def mod4 (in 9 @name mod4 @modulator 4)))
            (list))
          ;; a voice that runs before its first trigger counts its first sample as
          ;; a silent hit (every latch starts as on a retrigger), muted until a real one
          (list '(defmacro onset-of (trig)
                   (make-history previous)
                   (def high (gt trig 0.5))
                   (def rising (* high (lte (read-history previous) 0.5)))
                   (write-history previous high)
                   rising)
                '(def hit-real (onset-of trigger))
                '(make-history booted)
                '(def boot (lt (read-history booted) 0.5))
                '(write-history booted 1)
                '(def hit (max hit-real boot))
                '(make-history ever-hit)
                '(def heard (max hit-real (read-history ever-hit)))
                '(write-history ever-hit heard))))

(def vel-forms ()
  (list '(def vel (clip velocity 0.0001 1))
        '(def vgain (exp (* vel_amt 1.5 (log vel))))
        '(def vsnap (exp (* vel_amt 0.5 (log vel))))
        '(def vbend (exp (* vel_amt 0.5 (log vel))))
        '(def ratio (* tune (clip (/ pitch 261.6255653) 0.25 4)))
        '(def bendv (* bend vbend))))

(def mix-forms (v bs variant)
  (let ((clip-part (find-part (dict :parts (get v :parts)) :clipper nil))
        (drum-terms (section bs :drum)) (post (section bs :post)))
    (let ((clip-b (if clip-part (nth bs (index-of-part v clip-part)) nil)))
      (append (list (df 'dr (plus drum-terms)))
              (if clip-b ((get clip-b :mix) 'dr) (list (df 'drum 'dr)))
              (list (df 'post (plus post)))))))

(def index-of-part (v p) (gen/index-of (get v :parts) p))

;; The training voice's forms.
(def voice-forms (v)
  (let ((bs (builds v :static)) (P (segment-count v)) (H (get v :hidden)) (Z (get v :latent)) (L (get v :layout)))
    (let ((body (concat-all (list
                  (inputs-forms false)
                  (list (layout-form v))
                  (list :knobs)
                  (list `(def z (event-hold (latent-table @name codes @dim ,Z front-z) hit))
                        `(def W1 (tensor @shape ,(rng (str "[" Z " " H "]")) @file "w1.json" @learnable true))
                        `(def b1 (tensor @shape ,(shape H) @file "b1.json" @learnable true @init zeros))
                        `(def W2 (tensor @shape ,(rng (str "[" H " " H "]")) @file "w2.json" @learnable true))
                        `(def b2 (tensor @shape ,(shape H) @file "b2.json" @learnable true @init zeros))
                        `(def W3 (tensor @shape ,(rng (str "[" H " " P "]")) @file "w3.json" @learnable true @init small))
                        `(def b3 (tensor @shape ,(shape P) @file "b3.json" @learnable true @init zeros))
                        `(def u (param-row (+ (matmul (tanh (+ (matmul (tanh (+ (matmul z W1) b1)) W2) b2)) W3) b3) @layout ,L)))
                  (vel-forms)
                  (list `(defmacro decoded (name) (seg u ,L name))
                        '(defmacro at-hit (x) (latch x hit)))
                  (if (find-part (dict :parts (get v :parts)) :record-lp nil) (list) (no-record-lp :static))
                  (section bs :prelude)
                  (section bs :hit)
                  (list '(def t (max 0 (- (hit-time hit) T0))))
                  (section bs :stroke)
                  (section bs :dsp)
                  (mix-forms v bs :static)
                  (list '(def gx (clip (/ (- t (- EE FD)) FD) 0 1))
                        '(def gate-env (- 1 (* gx gx (- 3 (* 2 gx)))))
                        '(def voice-raw (declick (row-out (* LV gate-env (+ drum post))) hit))
                        '(def voice-out (* heard voice-raw))
                        '(out voice-out 1 @name left)
                        '(out voice-out 2 @name right))))))
      (splice-knobs body))))

;; Replace the :knobs marker with the params the forms read.
(def splice-knobs (forms)
  (let ((k (gen/index-of forms :knobs)) (others (filter (lambda (f) (not (= f :knobs))) forms)))
    (append (gen/take forms k) (knob-forms others) (rest (drop-n forms k)))))

(def drop-n (xs n) (map (lambda (i) (nth xs i)) (range n (len xs))))

;; The Morph instrument's forms: START (z1..z4) to END (b1..b4) over `morph` ms,
;; re-decoded every 128-sample hop. START = END reproduces the training voice.
(def morph-forms (v)
  (let ((bs (builds v :morph)) (P (segment-count v)) (H (get v :hidden)) (L (get v :layout)) (Z (get v :latent)))
    (let ((body (concat-all (list
                  (inputs-forms true)
                  (list (layout-form v))
                  (list :knobs)
                  (map (lambda (d) `(param ,(symbol "z" (+ d 1)) @default 0 @min -3 @max 3 @mod true @mod-mode additive)) (range Z))
                  (map (lambda (d) `(param ,(symbol "b" (+ d 1)) @default 0 @min -3 @max 3 @mod true @mod-mode additive)) (range Z))
                  (list '(param morph @default 400 @min 10 @max 4000 @unit ms)
                        '(param curve @default 0 @min -1 @max 1)
                        '(param amount @default 1 @min 0 @max 1)
                        '(param follow @default 1 @min 0 @max 1))
                  (morph-clock-forms Z)
                  (list '(def z (event-hold zt dclk))
                        `(def W1 (tensor @shape ,(rng (str "[" Z " " H "]")) @file "w1.json"))
                        `(def b1w (tensor @shape ,(shape H) @file "b1.json"))
                        `(def W2 (tensor @shape ,(rng (str "[" H " " H "]")) @file "w2.json"))
                        `(def b2w (tensor @shape ,(shape H) @file "b2.json"))
                        `(def W3 (tensor @shape ,(rng (str "[" H " " P "]")) @file "w3.json"))
                        `(def b3w (tensor @shape ,(shape P) @file "b3.json"))
                        '(def u (+ (matmul (tanh (+ (matmul (tanh (+ (matmul z W1) b1w)) W2) b2w)) W3) b3w)))
                  (vel-forms)
                  (list '(def out-trim 0.5))
                  (morph-macro-forms L)
                  (if (find-part (dict :parts (get v :parts)) :record-lp nil) (list) (no-record-lp :morph))
                  (section bs :prelude)
                  (section bs :hit)
                  (section bs :fixed)
                  (list '(def t (max 0 (- tm T0))) '(def act (gte tm T0)))
                  (section bs :stroke)
                  (section bs :dsp)
                  (mix-forms v bs :morph)
                  (list '(def gx (clip (/ (- t (- emax-v FD)) FD) 0 1))
                        '(def gate-env (- 1 (* gx gx (- 3 (* 2 gx)))))
                        '(def voice-raw (declick (sum (* LV gate-env (+ drum post))) hit))
                        '(def voice-out (* heard voice-raw))
                        '(out voice-out 1 @name left)
                        '(out voice-out 2 @name right)
                        '(use-defmacro voice-amp)
                        '(out (voice-amp 0 voice-out voice-out) 3 @name amp @amp true))))))
      (splice-knobs body))))

(def morph-clock-forms (Z)
  (append
    (list '(make-history age-n)
          '(make-history armed)
          '(def played (max hit (read-history armed)))
          '(write-history armed played)
          '(def n (gswitch hit 0 (min (* 8 samplerate) (+ 1 (read-history age-n)))))
          '(write-history age-n n)
          '(def tm (gswitch played (/ n samplerate) 8))
          '(def isr (/ 1 samplerate))
          '(def morph-s (* 0.001 (clip morph 10 4000)))
          '(def hop-s (/ 128 samplerate))
          '(def p (clip (/ (+ tm hop-s) morph-s) 0 1))
          '(def w (* (clip amount 0 1) (exp (* (exp (* (clip curve -1 1) 1.3862944)) (log (max p 0.000000001))))))
          '(def tick (eq (% n 128) 0))
          '(def morphing (* (gt amount 0) (lt tm (+ morph-s 0.003))))
          '(def dclk (max hit (* tick morphing played))))
    ;; START (z) and END (b) as one-hot sums, then the morph position between them
    (map (lambda (d) `(def ,(symbol "oh" d) (tensor @shape ,(rng (str "[1 " Z "]"))
                                            @data ,(rng (str "[" (apply str (interpose " " (map (lambda (k) (if (= k d) "1" "0")) (range Z)))) "]")))))
         (range Z))
    (list `(def zA (+ ,@(map (lambda (d) `(* (clip (mod ,(symbol "z" (+ d 1))) -3 3) ,(symbol "oh" d))) (range Z))))
          `(def zB (+ ,@(map (lambda (d) `(* (clip (mod ,(symbol "b" (+ d 1))) -3 3) ,(symbol "oh" d))) (range Z))))
          '(def zt (+ zA (* w (- zB zA)))))))

(def morph-macro-forms (L)
  (list `(defmacro decoded (name) (seg u ,L name))
        '(make-history last-clk)
        '(def clk-n (gswitch dclk n (read-history last-clk)))
        '(write-history last-clk clk-n)
        '(def frac (clip (/ (- n clk-n) 128) 0 1))
        '(def fo (clip follow 0 1))
        '(defmacro ramped (x shape)
           (def target (latch x dclk))
           (make-tensor-history held @shape shape)
           (def from (latch (gswitch hit target (read-tensor-history held)) dclk))
           (write-tensor-history held target)
           (+ from (* frac (- target from))))
        '(defmacro falling (x shape)
           (def decoded-now (latch x dclk))
           (make-tensor-history held @shape shape)
           (def previous (read-tensor-history held))
           (def target (latch (gswitch hit decoded-now (+ decoded-now (* fo (- (min previous decoded-now) decoded-now)))) dclk))
           (write-tensor-history held target)
           (def from (latch (gswitch hit target previous) dclk))
           (+ from (* frac (- target from))))
        '(defmacro at-hit (x) (latch x hit))
        '(defmacro decaying (rate start shape)
           (make-tensor-history st @shape shape)
           (def cur (gswitch hit (exp (* -1 (max 0 (- tm start)) rate)) (read-tensor-history st)))
           (write-tensor-history st (gswitch (gte tm start) (* cur (exp (* -1 rate isr)))
                                             (exp (* -1 (max 0 (- (+ tm isr) start)) rate))))
           cur)))

;; layout.json: the voice's segments in order, with their part, count and
;; range, the parts' kinds and options a seeder or report needs.
(def jq (x) (source (str x)))

;; :modes -> "modes"
(def kind-name (k) (let ((t (source k))) (substring t 1 (len t))))

;; What a field is for, by part kind: `freq` fields are frozen in refine A
;; (frequencies move slowly and trap the fit), `time` fields too (a stroke time
;; is a discrete choice), `level` fields at their lower bound silence the part
;; (the superset check and seeding new parts). Others are neutral.
(def roles '((:modes "f" freq) (:modes "amp" level) (:strikes "xamp" level) (:bands "wl" level)
             (:hiss "hl" level) (:crackle "cr_lvl" level) (:events "ev_lvl" level) (:template "lvl" level)
             (:wash "l" level) (:restrike "t" time) (:restrike "lvl" level) (:template "lvl2" level)
             (:wash "l2" level) (:strings "amp" level) (:rattle "l" level)))

(def role-of (kind field)
  (let ((hits (filter (lambda (r) (and (= (first r) kind) (= (nth r 1) field))) roles)))
    (if (empty? hits) "neutral" (source (nth (first hits) 2)))))

(def seg-json (p k)
  (let ((f (nth (get p :segments) k)) (field (first (nth (get p :fields) k))))
    (str "{" (jq "name") ": " (jq (source (first f))) ", " (jq "part") ": " (jq (source (get p :id)))
         ", " (jq "kind") ": " (jq (kind-name (get p :kind))) ", " (jq "field") ": " (jq field)
         ", " (jq "role") ": " (jq (role-of (get p :kind) field)) ", " (jq "count") ": " (nth f 1)
         ", " (jq "scale") ": " (jq (source (nth f 2)))
         (if (> (len f) 3) (str ", " (jq "lo") ": " (nth f 3) ", " (jq "hi") ": " (nth f 4)) "")
         "}")))

(def part-json (p)
  (str "{" (jq "id") ": " (jq (source (get p :id))) ", " (jq "kind") ": " (jq (kind-name (get p :kind)))
       ", " (jq "follow-note") ": " (if (= (get p :follow-note) false) "false" "true")
       ", " (jq "strokes") ": " (if (get p :strokes) "true" "false")
       ", " (jq "on") ": " (if (get p :on) (jq (source (get p :on))) "null")
       ", " (jq "knob") ": " (if (get p :knob) (jq (source (get p :knob))) "null")
       ", " (jq "bus") ": " (if (get p :bus) (jq (kind-name (get p :bus))) "null") "}"))

(def layout-json (v)
  (str "{" (jq "name") ": " (jq (get v :name)) ", " (jq "layout") ": " (jq (source (get v :layout)))
       ", " (jq "width") ": " (segment-count v)
       ",\n " (jq "lineage") ": " (to-json (get v :lineage))
       ",\n " (jq "parts") ": [" (apply str (interpose ", " (map part-json (get v :parts)))) "]"
       ",\n " (jq "segments") ": [\n  "
       (apply str (interpose ",\n  " (concat-all (map (lambda (p) (map (lambda (k) (seg-json p k)) (range (len (get p :segments)))))
                                                      (get v :parts)))))
       "]}\n"))

(def interpose (sep xs)
  (concat-all (map (lambda (k) (if (= k 0) (list (nth xs k)) (list sep (nth xs k)))) (range (len xs)))))

;; Write DIR/voice.lisp, DIR/morph.lisp and DIR/layout.json (needs the
;; `write-text-file` native: `eseq lisp run`). Returns the error list, or nil.
(def emit (v dir)
  (let ((es (errors v)))
    (if (not (empty? es)) es
      (let ((head (str ";; generated by eseq.modal.train from the voice " (get v :name) "; edit the patch, not this file\n")))
        (write-text-file (str dir "/voice.lisp") (str head (pp-forms (voice-forms v))))
        (write-text-file (str dir "/morph.lisp") (str head (pp-forms (morph-forms v))))
        (write-text-file (str dir "/layout.json") (layout-json v))
        nil))))

;; ── experiments: data + seeding + recipe + shipping in the patch ──────────
;; (t/experiment "break-snare-v3" :voice v :data (t/data ...) :seed (t/seed ...)
;;   :train (t/recipe ...) :ship (t/ship ...))  then (t/emit-experiment e DIR)
;; writes DIR/{voice,morph}.lisp, layout.json and experiment.json; the runner
;; (tools/modal/train/run.py DIR) builds the dataset, seeds, trains, scores,
;; compares with the parent and installs the instrument.

;; Where the samples come from and how they are cut.
;;   packs       sample-library pack names (Save Region's packs)
;;   tags        tag sets: a sample with every tag of one set is in
;;   keep-crops  a dataset dir whose samples (by hash) keep its exact crops
;;   crop        :first-hit (2 ms before the first hit) or :none
;;   frames      training length in samples (44.1 kHz)
(def data (&key (packs nil) (tags nil) (keep-crops nil) (crop :first-hit) (frames 32768))
  (dict :packs packs :tags tags :keep-crops keep-crops :crop crop :frames frames))

;; Where the starting rows come from, per sample, in order of preference:
;;   parent   a previous experiment dir (or run dir): its fitted rows, by segment
;;            name, for the samples and segments it has
;;   parent-voice  the parent's training voice when the parent is a bare run dir
;;            (an experiment dir has its own voice.lisp): for the superset check
;;   rows     a rows.json to take the same way
;;   analyze  for samples neither has: :drum (dgen analyze-drums + drum-rows
;;            snare1, then the parts' seeders) or :neutral
;;   level    (part factor) pairs scaling a part's seeded levels (seeds too hot)
;; Segments a sample has no source for start neutral, levels silent.
(def seed (&key (parent nil) (parent-voice nil) (rows nil) (analyze :drum) (level nil))
  (dict :parent parent :parent-voice parent-voice :rows rows :analyze analyze :level level))

;; Training stages, in order.
(def refine (&key (steps 400) (lr 0.02) (freeze :roles) (wave 0) (refit :all))
  (dict :stage "refine" :steps steps :lr lr :freeze freeze :wave wave :refit refit))
(def pick (&key (band 0.3) (mr 1)) (dict :stage "pick" :band band :mr mr))
(def pretrain (&key (steps 3000) (lr 0.003)) (dict :stage "pretrain" :steps steps :lr lr))
(def path (lam &key (steps 4000) (lr 0.002) (seed 1)) (dict :stage "path" :lambda lam :steps steps :lr lr :seed seed))
(def recipe (&rest stages) stages)

;; The instrument: name, library folder, Morph twin, which knobs go on the panel
;; next to the stock ones.
(def ship (name &key (folder "Research") (morph true) (extra-knobs '(rhythm metal)))
  (dict :name name :folder folder :morph morph :extra-knobs extra-knobs))

(def experiment (name &key (voice nil) (data nil) (seed nil) (train nil) (ship nil) (notes ""))
  (dict :kind :experiment :name name :voice voice :data data :seed seed :train train :ship ship :notes notes))

;; JSON: dicts (keyword keys), lists, strings, numbers, keywords / symbols (as
;; strings), true / false / nil.
(def to-json (x)
  (if (= x nil) "null"
    (if (= x true) "true"
      (if (= x false) "false"
        (if (number? x) (str x)
          (if (string? x) (source x)
            (if (map? x) (str "{" (apply str (interpose ", " (map (lambda (k) (str (jq (kind-name k)) ": " (to-json (get x k)))) (keys x)))) "}")
              (if (list? x) (str "[" (apply str (interpose ", " (map to-json x))) "]")
                (let ((t (source x))) (jq (if (= (substring t 0 1) ":") (substring t 1 (len t)) t)))))))))))

(def emit-experiment (e dir)
  (let ((v (get e :voice)))
    (let ((es (emit v dir)))
      (if es es
        (do (write-text-file (str dir "/experiment.json")
              (str (to-json (dict :name (get e :name) :voice (get v :name) :notes (get e :notes)
                                  :data (get e :data) :seed (get e :seed) :train (get e :train) :ship (get e :ship)))
                   "\n"))
            nil)))))
