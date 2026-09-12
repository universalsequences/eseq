;; PM Hi-Hat / reduced dispersive plate, identified from Donit's cymbal recordings.
;; Six frequency regions, each with three paths and an orthogonal junction.
;; Eight resolved resonances carry the bell/foot modes; dense plate modes live
;; in the delay network. No PCM, recorded phase or sampled envelope is used.
;; Rebuild and validation: tools/pm-cymbals/README.md.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
(param character @group voicing @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
(param size @group body @default 1 @min 0.5 @max 2 @mod true @mod-mode additive)
(param decay @group body @default 1 @min 0.2 @max 3 @mod true @mod-mode additive)
(param damping @group body @default 1 @min 0.25 @max 3 @mod true @mod-mode additive)
(param hardness @group stick @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
(param bell @group body @default 0.5 @min 0 @max 2 @mod true @mod-mode additive)
(param wash @group body @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param touch @group contact @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param color @group output @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param width @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @group output @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param tracking @group tuning @default 0 @min 0 @max 1)
(param openness @group contact @default 0 @min 0 @max 1 @mod true @mod-mode additive)
;; Calibration SHA256: 6fd97ce4ae5bed981273335f9ffd4e4b33d5f4da91d967a60d51bca2a700d511
(def closed_material (tensor @shape [24] @data [
  0.1000000642 4.093581456 9.407008561 10.40769251 11.9600084 29.56811696 0.0006 0
  2.598892251 5.38647153 5.970474627 6.307039862 11.19308993 21.5821859 0.0006 0
  1.026189432 4.535519641 9.024543714 37.6089583 12.2358448 23.18857392 0.0006 0
]))
(def closed_band_gains (tensor @shape [54] @data [
  0.910007855 1.213534745e-10 9.635828428e-11 1.046423171e-10 13.04791077 9.719949747e-11 3.060279002 2.936382028 0.2702949616 2.354562486 1.816398608e-10 1.746957042e-10 2.643929909 26.45699175 2.921864529 88.73445507 88.64214267 52.69908312
  1.909263792 1.196262541e-10 1.250993496 1.097779931e-10 11.30558197 1.877805008 8.9444268e-11 8.117758411e-11 2.617059828 1.31433425e-10 6.720222511 1.717620117e-10 13.42626014 19.0072125 17.98775573 11.98331922 111.538873 83.45571715
  3.264784928 4.793354351e-09 1.293771164 2.232288285 12.8346925 1.460713782e-08 1.357743326e-08 1.370477824e-08 5.130146779 12.72618497 3.81152611e-08 12.49679487 20.86031784 15.32236438 12.05083299 0.2417544725 111.1716047 98.55531333
]))
(def closed_mode_frequencies (tensor @shape [24] @data [
  333.5526517 407.4055993 3210.708171 3891.799613 4418.903047 4551.31066 4945.717693 5882.438382
  332.8732228 3268.351376 4108.253156 4426.373367 4848.581127 5256.975968 5428.755151 11102.89
  147.451472 195.9892443 432.6171635 470.2968839 4876.768951 4994.39193 6188.22589 11528.60728
]))
(def closed_mode_rates (tensor @shape [24] @data [
  3.694158369 7.498339634 4.298807354 6.397580173 6.468120072 6.261309304 7.448504602 8.083614321
  6.845023577 12.79346184 9.932728889 8.369350724 12.82559974 14.28722251 12.38951024 14.87227363
  5.463724889 5.524364381 6.90200724 10.17717534 13.95661026 14.25206095 12.70117159 14.04994082
]))
(def closed_mode_gains (tensor @shape [24] @data [
  0.002967792112 7.847939266e-13 0.001666741517 0.004975545912 7.819326809e-13 7.80315685e-13 7.868764283e-13 7.905908398e-13
  0.02843142992 0.03927898798 0.02941168157 0.01962415945 0.06504901329 0.06715372019 0.04691098451 0.1255809921
  0.002466247442 1.038212284e-10 1.091681621e-10 0.004346373423 0.04400688687 0.04576685935 0.04701165599 0.03287708592
]))
(def open_material (tensor @shape [24] @data [
  0.1000083876 3.208525977 2.875632297 5.361215812 6.785696296 26.3712656 0.008 0
  8.710868201 0.3673505556 0.4576465511 0.7260978952 1.991401011 30.60271264 0.0006 0.3
  9.710992712 4.718200415 3.024609927 0.100000542 1.373674532 13.6614321 0.0025 1
]))
(def open_band_gains (tensor @shape [54] @data [
  0.07609490168 1.618275865e-25 0.06356256939 2.002332252e-35 7.583602847e-35 0.8869582952 5.051035767e-37 2.29996667 6.089049521e-37 0.260874499 1.271311525e-16 4.064784602e-07 5.468450464e-24 3.333620221e-09 13.2264543 3.104370798e-24 25.1156078 2.371816162e-11
  1.423254511e-10 9.053195656e-11 0.8211957446 7.991748308e-11 1.389471161 6.816696128e-11 0.3285959723 7.794880284e-11 0.9744459403 0.0721962302 1.484983953e-10 6.03089445 1.912104116e-10 2.383689349e-10 37.63266468 5.146121921e-10 52.56164918 9.059009575e-10
  1.312682902e-11 0.131848261 1.425122907e-11 1.248055517e-11 0.5177046149 2.711715038 2.451566731 3.485722104 1.241199871 0.8019744789 3.693833225e-11 4.757674951e-11 3.465399412 14.71057365 0.3427562744 9.142503437e-11 9.752455479 38.72869249
]))
(def open_mode_frequencies (tensor @shape [24] @data [
  295.5978281 2429.189313 2504.045381 3390.75966 3443.218079 4284.760958 4511.814886 6802.740067
  1473.013988 3254.734494 3351.42366 3566.714111 4457.208353 4805.746814 5520.38198 7397.52184
  3531.542297 3638.509971 3694.09167 7440.83948 9209.160731 10882.86356 11081.84305 11461.42896
]))
(def open_mode_rates (tensor @shape [24] @data [
  2.916345453 4.454675893 2.035463985 2.511827499 3.7780147 4.504959652 8.173517216 5.739690545
  0.5562282701 1.708719504 1.126578212 1.01933442 1.662304818 1.404329003 1.394737462 2.922305057
  2.414764289 1.644630456 0.1223904112 1.391838475 0.1 0.1 0.3883806616 0.3586642569
]))
(def open_mode_gains (tensor @shape [24] @data [
  0.007664127318 0.00993434524 0.01085096505 0.01406304433 0.02211962271 0.01592292826 0.04652939624 0.002071321434
  0.002103701736 0.01869412528 0.0177626479 0.01109947887 0.02475677406 0.01373102781 0.009803490188 0.02857415528
  0.06414189505 0.03831361664 0.01391718838 0.01742345324 0.01282504725 0.01628263858 0.01429521197 0.01408703759
]))

(defmacro cymbal-smooth (target ms)
  (make-history previous)
  (make-history ready)
  (def pole (exp (/ -1 (* 0.001 ms samplerate))))
  (def value (gswitch (read-history ready) (mix target (read-history previous) pole) target))
  (write-history previous value)
  (write-history ready 1)
  value)
(defmacro cymbal-pole (input pole)
  (make-history previous)
  (def value (mix input (read-history previous) pole))
  (write-history previous value)
  value)
(make-history last_gate)
(def held (gt gate 0.5))
(def onset (max (gt trigger 0.5) (* held (lte (read-history last_gate) 0.5))))
(write-history last_gate held)
(def tick (max onset (eq (accum 1 0 0 16) 0)))
(def strength (latch (clip velocity 0 1) onset))
(def scale (event-hold (cymbal-smooth (/ (clip (mod size) 0.5 2)
  (pow (/ (clip pitch 65.406391 1046.50226) 261.625565) (clip tracking 0 1))) 12) tick))
(def decay_v (event-hold (cymbal-smooth (clip (mod decay) 0.2 3) 8) tick))
(def damping_v (event-hold (cymbal-smooth (clip (mod damping) 0.25 3) 8) tick))
(def touch_v (event-hold (cymbal-smooth (clip (mod touch) 0 1) 2) tick))
(def character_v (event-hold (cymbal-smooth (clip (mod character) 0 1) 12) tick))
(def closed_row (* character_v 2))
(def closed_lo (floor closed_row))
(def closed_hi (min 2 (+ closed_lo 1)))
(def closed_mix (- closed_row closed_lo))
(def closed_material_v (mix (gather closed_material (+ (iota 8) (* closed_lo 8))) (gather closed_material (+ (iota 8) (* closed_hi 8))) closed_mix))
(def closed_band_gains_v (mix (gather closed_band_gains (+ (iota 18) (* closed_lo 18))) (gather closed_band_gains (+ (iota 18) (* closed_hi 18))) closed_mix))
(def closed_mode_frequencies_v (mix (gather closed_mode_frequencies (+ (iota 8) (* closed_lo 8))) (gather closed_mode_frequencies (+ (iota 8) (* closed_hi 8))) closed_mix))
(def closed_mode_rates_v (mix (gather closed_mode_rates (+ (iota 8) (* closed_lo 8))) (gather closed_mode_rates (+ (iota 8) (* closed_hi 8))) closed_mix))
(def closed_mode_gains_v (mix (gather closed_mode_gains (+ (iota 8) (* closed_lo 8))) (gather closed_mode_gains (+ (iota 8) (* closed_hi 8))) closed_mix))

(def open_row (* character_v 2))
(def open_lo (floor open_row))
(def open_hi (min 2 (+ open_lo 1)))
(def open_mix (- open_row open_lo))
(def open_material_v (mix (gather open_material (+ (iota 8) (* open_lo 8))) (gather open_material (+ (iota 8) (* open_hi 8))) open_mix))
(def open_band_gains_v (mix (gather open_band_gains (+ (iota 18) (* open_lo 18))) (gather open_band_gains (+ (iota 18) (* open_hi 18))) open_mix))
(def open_mode_frequencies_v (mix (gather open_mode_frequencies (+ (iota 8) (* open_lo 8))) (gather open_mode_frequencies (+ (iota 8) (* open_hi 8))) open_mix))
(def open_mode_rates_v (mix (gather open_mode_rates (+ (iota 8) (* open_lo 8))) (gather open_mode_rates (+ (iota 8) (* open_hi 8))) open_mix))
(def open_mode_gains_v (mix (gather open_mode_gains (+ (iota 8) (* open_lo 8))) (gather open_mode_gains (+ (iota 8) (* open_hi 8))) open_mix))
(def openness_v (event-hold (cymbal-smooth (clip (mod openness) 0 1) 2) tick))
(def material (mix closed_material_v open_material_v openness_v))
(def band_gains (mix closed_band_gains_v open_band_gains_v openness_v))
(def mode_frequencies (mix closed_mode_frequencies_v open_mode_frequencies_v openness_v))
(def mode_rates (mix closed_mode_rates_v open_mode_rates_v openness_v))
(def mode_gains (mix closed_mode_gains_v open_mode_gains_v openness_v))
(def base_rate0 (sample material 0))
(def base_rate1 (sample material 0.125))
(def base_rate2 (sample material 0.25))
(def base_rate3 (sample material 0.375))
(def base_rate4 (sample material 0.5))
(def base_rate5 (sample material 0.625))
(def base_contact_s (sample material 0.75))
(def base_direct (latch (sample material 0.875) tick))

;; Openness is a live loss condition, not an output envelope. Closed-hat
;; coefficients include contact losses; touch adds a choke to every wave path
;; and resolved resonance. Gate-off deliberately leaves percussion ringing.
(def contact_loss (* 180 touch_v touch_v))

;; Finite stick contact: resolved modes receive the compression impulse;
;; unresolved modes receive the compact, stochastic rough-contact force.
;; Randomness exists only while the stick contacts the metal; all subsequent
;; sound is the free response of the passive body.
(def hardness_v (event-hold (clip (mod hardness) 0 1) tick))
(def contact_s (event-hold (* base_contact_s (pow 2 (* 2 (- 0.5 hardness_v)))) tick))
(def age (accum (/ 1 samplerate) onset 0 100000))
(def pulse_phase (clip (/ age contact_s) 0 1))
(def friction (* (noise) (sin (* pi pulse_phase)) (lt age contact_s)))
(def force_pole (latch (exp (/ -1 (* samplerate 0.000025 (pow 2 (* 3 (- 0.5 hardness_v)))))) tick))
(def force (cymbal-pole (cymbal-pole
  (* strength friction 0.23) force_pole) force_pole))
(def point_force (cymbal-pole (cymbal-pole (* onset strength) force_pole) force_pole))
;; One lane per original wave path; six independent three-port junctions.
(def path_lengths (tensor @shape [18] @data [109 461 1797 130 549 2141 153 650 2535 177 751 2929 204 864 3372 234 991 3864]))
(def path_regions (tensor @shape [18] @data [0 0 0 1 1 1 2 2 2 3 3 3 4 4 4 5 5 5]))
(def path_lanes (tensor @shape [18] @data [0 1 2 0 1 2 0 1 2 0 1 2 0 1 2 0 1 2]))
(def region_first (tensor @shape [6] @data [0 3 6 9 12 15]))
(def region_second (tensor @shape [6] @data [1 4 7 10 13 16]))
(def region_third (tensor @shape [6] @data [2 5 8 11 14 17]))
(def force_weights (tensor @shape [18] @data [0.57735026919 -0.57735026919 0.57735026919 0.57735026919 -0.57735026919 0.57735026919 0.57735026919 -0.57735026919 0.57735026919 0.57735026919 -0.57735026919 0.57735026919 0.57735026919 -0.57735026919 0.57735026919 0.57735026919 -0.57735026919 0.57735026919]))
(def region_rates (+ (/ (* (gather material path_regions) (pow damping_v (/ path_regions 5))) decay_v) contact_loss))
(def path_samples (max 8 (* path_lengths scale (/ samplerate 48000))))
(def integer_delays (latch (- (floor path_samples) 2) tick))
(def fractions (+ 1 (- path_samples (floor path_samples))))
(def allpass_a (latch (/ (- 1 fractions) (+ 1 fractions)) tick))
(def path_loss (latch (exp (* (- region_rates) (/ path_samples samplerate))) tick))
(make-tensor-history path_outgoing @shape [18])
(make-tensor-history path_ap_x @shape [18])
(make-tensor-history path_ap_y @shape [18])
(def incoming (delay (read-tensor-history path_outgoing) integer_delays @max-delay 88000))
(def arrival (- (+ (* allpass_a incoming) (read-tensor-history path_ap_x)) (* allpass_a (read-tensor-history path_ap_y))))
(write-tensor-history path_ap_x incoming)
(write-tensor-history path_ap_y arrival)
(def damped (* path_loss arrival))
;; Reduced relative shell motion and a unilateral contact junction.
;; q/v use energy-normalized spring coordinates. The free rotation contracts;
;; projection only removes potential energy. On impact the two-port scattering
;; has eigenvalues 1 and -restitution, so it cannot create stored energy.
;; A normalized projection couples this one contact to all six wave regions.
(make-history shell_q)
(make-history shell_v)
(def shell_angle (/ (* twopi 195) (* samplerate scale)))
(def shell_radius (exp (/ (- (+ 2 (* 12 (- 1 openness_v)))) samplerate)))
(def shell_c (latch (* shell_radius (cos shell_angle)) tick))
(def shell_s (latch (* shell_radius (sin shell_angle)) tick))
(def shell_free_q (+ (* shell_c (read-history shell_q)) (* shell_s (read-history shell_v))))
(def shell_free_v (+ (- (* shell_c (read-history shell_v)) (* shell_s (read-history shell_q))) (* onset strength 0.05)))
(def shell_gap (latch (* 0.08 openness_v openness_v) tick))
(def incoming_contact (* 0.408248290463863 (sum (gather damped region_first))))
(def shell_impact (* (gt shell_free_q shell_gap) (gt shell_free_v incoming_contact)))
(def restitution (+ 0.2 (* openness_v 0.65)))
(def collision_a (latch (* 0.5 (- 1 restitution)) tick))
(def collision_b (latch (* 0.5 (+ 1 restitution)) tick))
(def outgoing_contact (+ (* collision_a incoming_contact) (* collision_b shell_free_v)))
(def collision_velocity (+ (* collision_b incoming_contact) (* collision_a shell_free_v)))
(def contact_delta (* shell_impact (- outgoing_contact incoming_contact) 0.408248290463863))
(write-history shell_q (min shell_free_q shell_gap))
(write-history shell_v (gswitch shell_impact collision_velocity shell_free_v))

;; Unresolved microscopic contacts randomize reflection phase while preserving
;; wave energy. Noise controls an orthogonal matrix; it is never added to the
;; ringing signal. A physical-rate pole bounds the contact correlation time.
(def contact_memory (cymbal-pole shell_impact (exp (/ -1 (* samplerate 0.0007)))))
(def contact_fraction (max (latch (pow (- 1 openness_v) 8) tick) contact_memory))
(def roughness_pole (exp (/ (* -1 twopi 4800) samplerate)))
(def roughness (cymbal-pole (- (* 2 (noise)) 1) roughness_pole))
;; Bipolar uniform noise has variance 1/3; normalize the filtered variance.
(def roughness_scale (sqrt (/ (* 3 (+ 1 roughness_pole)) (- 1 roughness_pole))))
(def scatter_u (+ 1 (* 0.8 contact_fraction roughness roughness_scale)))
(def scatter_norm (/ 2 (+ 2 (* scatter_u scatter_u))))

(def scattered (+ damped (* contact_delta (eq path_lanes 0))))
(def junction (* scatter_norm (+ (gather scattered region_first) (* scatter_u (gather scattered region_second)) (gather scattered region_third))))
(write-tensor-history path_outgoing (+ (- (* (gather junction path_regions) (gswitch (eq path_lanes 1) scatter_u 1)) scattered) (* force force_weights)))
(def plates (+ (* force base_direct) (* (gather scattered region_first) 0.485823499594) (* (gather scattered region_second) -0.147516199622) (* (gather scattered region_third) -0.268275790313)))

(def radiation_hz (tensor @shape [18] @data [100 135.944938584 184.810263266 251.240198894 341.548334086 464.317673008 631.216375405 858.106713878 1166.55264517 1585.86927702 2155.90901467 2930.84918593 3984.3411258 5416.51009645 7363.47132402 10010.2665691 13608.4507395 18500]))
(def region_ids (tensor @shape [18] @data [0 0 0 1 1 1 2 2 2 3 3 3 4 4 4 5 5 5]))
(def plate_vector (gather plates region_ids))
(def filter_hz (min (/ radiation_hz scale) (* samplerate 0.43)))
(def filter_g (tan (* pi (/ filter_hz samplerate))))
(def filter_a1 (/ 1 (+ 1 (* filter_g (+ filter_g 0.3125)))))
(def filter_a2 (* filter_g filter_a1))
(def filter_a3 (* filter_g filter_a2))
(make-tensor-history filter_ic1 @shape [18])
(make-tensor-history filter_ic2 @shape [18])
(def ic1 (read-tensor-history filter_ic1))
(def ic2 (read-tensor-history filter_ic2))
(def a1 (latch filter_a1 tick))
(def a2 (latch filter_a2 tick))
(def a3 (latch filter_a3 tick))
(def v3 (- plate_vector ic2))
(def v1 (+ (* a1 ic1) (* a2 v3)))
(def v2 (+ ic2 (* a2 ic1) (* a3 v3)))
(write-tensor-history filter_ic1 (- (* 2 v1) ic1))
(write-tensor-history filter_ic2 (- (* 2 v2) ic2))
(def radiation_bands v1)
(def frequencies (/ mode_frequencies scale))
(def omega (* twopi (/ (min frequencies (* samplerate 0.47)) samplerate)))
(def radius (exp (/ (- (+ (/ mode_rates decay_v) contact_loss)) samplerate)))
(def c (latch (* radius (cos omega)) tick))
(def s (latch (* radius (sin omega)) tick))
(def band (clip (/ (- (* samplerate 0.47) frequencies) (* samplerate 0.06)) 0 1))
;; Deconvolve the reference two-pole contact response, without recorded phase.
(def reference_pole (exp (/ -1 (* samplerate 0.000025))))
(def compensation (/ (+ (* (- 1 reference_pole) (- 1 reference_pole))
  (* 2 reference_pole (- 1 (cos omega)))) (* (- 1 reference_pole) (- 1 reference_pole))))
(make-tensor-history mode_r @shape [8])
(make-tensor-history mode_i @shape [8])
(def x (read-tensor-history mode_r))
(def y (read-tensor-history mode_i))
(def next_r (+ (- (* c x) (* s y)) (* point_force (latch (* band compensation) tick))))
(def next_i (+ (* s x) (* c y)))
(write-tensor-history mode_r next_r)
(write-tensor-history mode_i next_i)
(def resolved next_i)
(def color_v (event-hold (cymbal-smooth (clip (mod color) -1 1) 8) tick))
(def wash_v (cymbal-smooth (clip (mod wash) 0 2) 8))
(def bell_v (cymbal-smooth (clip (mod bell) 0 2) 8))
(def width_v (cymbal-smooth (clip (mod width) 0 1) 8))
(def gain_v (cymbal-smooth (clip (mod gain) 0 2) 8))
(def radiation (* radiation_bands (latch (* band_gains (pow (/ radiation_hz 2800) (* color_v 0.6))) tick)))
(def band_pan (tensor @shape [18] @data [0 0.303970632 -0.448276968 0.357120338 -0.0783818782 -0.241527623 0.434571783 -0.399351794 0.154367385 0.171700383 -0.407580422 0.429373855 -0.225633413 -0.0966237415 0.368128093 -0.446268656 0.290001144 0.0185930197]))
(def body_left (sum (* radiation (+ 1 (* width_v band_pan)))))
(def body_right (sum (* radiation (- 1 (* width_v band_pan)))))
(def modal_pan (tensor @shape [8] @data [0 0.4 -0.5 0.3 -0.2 0.5 -0.4 0.1]))
(def mode_signal (* resolved (latch mode_gains tick)))
(def bell_left (sum (* mode_signal (+ 1 (* width_v modal_pan)))))
(def bell_right (sum (* mode_signal (- 1 (* width_v modal_pan)))))
(out (* 0.65 gain_v (+ (* wash_v body_left) (* bell_v bell_left))) 1 @name left)
(out (* 0.65 gain_v (+ (* wash_v body_right) (* bell_v bell_right))) 2 @name right)
