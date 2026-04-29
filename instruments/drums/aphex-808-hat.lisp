; Aphex-inspired 808 Hi-Hat Synth
; Uses 6 square oscillators with modulated ratios, noise, and resonant filtering.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

(defmacro sq (freq)
  (scale (lt (phasor freq) 0.5) 0 1 -1 1))

(param decay_ms     @default 70   @min 2     @max 1500 @unit ms @mod true @mod-mode additive)
(param attack_ms    @default 1    @min 0.1   @max 100  @unit ms)
(param ratio_warp   @default 1.0  @min 0.1   @max 8    @mod true @mod-mode additive)
(param noise_mix    @default 0.3  @min 0     @max 1    @mod true @mod-mode additive)
(param cutoff       @default 8000 @min 200   @max 18000 @unit Hz @mod true @mod-mode additive)
(param resonance    @default 1.2  @min 0.1   @max 5    @mod true @mod-mode additive)
(param drive        @default 1.5  @min 1     @max 20   @mod true @mod-mode additive)
(param fm_amt       @default 0.0  @min 0     @max 1    @mod true @mod-mode additive)
(param crush        @default 0.0  @min 0     @max 1    @mod true @mod-mode additive)
(param gain         @default 0.2  @min 0     @max 1)

; ADSR for hat envelope (short decay, zero sustain)
(def env (adsr gate trigger attack_ms (mod decay_ms) 0.0 20))

; Base frequencies for 808 metallic sound
(def rw (mod ratio_warp))
(def f1 (* 245 rw))
(def f2 (* 306 rw))
(def f3 (* 384 rw))
(def f4 (* 522 rw))
(def f5 (* 800 rw))
(def f6 (* 1000 rw))

; FM Interaction
(def fm (* (sq (* f1 0.5)) (mod fm_amt) 2000))

(def s1 (sq (+ f1 fm)))
(def s2 (sq (+ f2 (* fm 0.7))))
(def s3 (sq (+ f3 (* fm 1.3))))
(def s4 (sq (+ f4 (* fm 0.9))))
(def s5 (sq (+ f5 (* fm 1.1))))
(def s6 (sq (+ f6 (* fm 0.5))))

; XOR-like metallic core
(def metallic (sign (+ s1 s2 s3 s4 s5 s6)))

; Blend with noise
(def source (mix metallic (noise) (mod noise_mix)))

; Saturation and Bitcrush-style logic
(def saturated (tanh (* source (mod drive))))

; Filter (High Pass for hats)
(def filt (biquad saturated (mod cutoff) (mod resonance) 1 1))

; Gain-aware output
(out (* filt env velocity gain) 1 @name audio)
