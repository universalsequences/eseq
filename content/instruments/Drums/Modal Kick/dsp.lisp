; Factory Modal Kick — the drums/modal-kick acoustic bass drum (modal
; synthesis: Bessel-tuned resonator banks on both heads, scalar (0,n)
; proxies air-coupled through the shell, felt Hertz beater, pitch drop,
; port/muffle/shell/click) with the factory kick post chain from
; Drums/808 Kick and Drums/909 Kick appended after the shaper:
;   - the BANK: the sc-filterbank (Sherman FB2) core with its cutoff on a
;     per-trigger decay envelope, keytrack + drive modes, reconstruction;
;   - a final HP/LP tone pair after the bank, exact bypass at the defaults.
; Both are no-ops at their defaults, so this boots up AS the modal kick.
; The bank/tone param names and ranges are the 808/909 ones verbatim, so
; the three kicks share one BANK + TONE surface.
;
; Engine notes (from drums/modal-kick): acoustic bass drum, modal-synthesis sibling of
; drums/modal-snare. Same engine idea: both heads are banks of 2-pole
; resonators tuned to the circular-membrane eigenmodes (Bessel zeros), the
; (0,n) "volume" modes of both heads are SCALAR proxies coupled two-way by
; the air trapped in the shell, and the tensor banks hold only the
; asymmetric (m>0) modes, coupled elementwise (the compiler only breaks
; feedback through REDUCES). What a kick does NOT need from the snare: wires,
; rim hoop, stroke/press expression. What it needs that the snare lacks:
;   - a FELT BEATER: same lumped-mass Hertz striker, but soft (hardness
;     ~0.0004 vs the stick's 0.004) so it stays on the head for several ms,
;     which low-passes the excitation — that contact time is the difference
;     between a click and a thud. `beater_size` spreads the hit over the
;     modes (a 2" felt on a 22" head is ~0.12 of the radius).
;   - a PITCH DROP (`bend`/`bend_time`): the head goes tension-nonlinear on a
;     hard hit, so the fundamental starts sharp and settles. Driven by the
;     peak-held beater force so it scales with velocity, on the fundamental
;     and the proxies (never inside the tensor feedback).
;   - a PORT hole in the resonant head that vents the cavity (scales the air
;     coupling down) and MUFFLE (a pillow on the batter: per-sample state
;     shrink weighted toward the upper modes).
;   - SHELL knock: two fixed inharmonic 2-poles fed by the beater force.
;   - CLICK: the beater's own contact transient, high-passed (contact mic).
; Bank is [4 4] = 16 modes per head (a near-center felt hit barely reaches
; the m>0 modes, so 36 was pointless): tables baked by scratchpad
; gen_kick_tables.py (numpy-only Bessel integrals): beater at r=0.12, mics
; at r=0.45 (batter) and r=0.35 (reso), strike weights = psi(x0)/modal-mass
; with NO baked tip (the spread is applied live from `beater_size`).

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def host_pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
(def vel (clip velocity 0 1))
(def vel-gain (sqrt vel))

; ── Params (names match modal-snare where the meaning is the same) ──────────
(param release @default 420 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)      ; batter fundamental T60
(param release2 @default 650 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)     ; reso fundamental T60
; A4/440 -> ~55 Hz batter fundamental
(param tune @default -36 @min -60 @max 12 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch2_ratio @default 1.15 @min 0.5 @max 2 @mod true @mod-mode additive @mod-depth-min -0.5 @mod-depth-max 0.5)
; partial spread: <1 compresses the overtones (air-loaded head), >1 stretches
(param stretch @default 0.85 @min 0.4 @max 1.6 @mod true @mod-mode additive @mod-depth-min -0.5 @mod-depth-max 0.5)
; per-mode decay tilt, T60_n = release * ratio_n^-tilt
(param tilt @default 1.3 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; beater: felt (soft, long contact) .. wood (hard, short contact)
(param beater_hard @default 0.0004 @min 0.00002 @max 0.01 @mod true @mod-mode additive @mod-depth-min -0.002 @mod-depth-max 0.002)
(param beater_speed @default 0.03 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)
; beater radius as a fraction of the head radius (spreads the hit over modes)
(param beater_size @default 0.12 @min 0.03 @max 0.45 @mod true @mod-mode additive @mod-depth-min -0.2 @mod-depth-max 0.2)
; tension-nonlinearity pitch drop: amount and settle time
(param bend @default 1.0 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param bend_time @default 40 @min 5 @max 400 @unit ms @mod true @mod-mode additive @mod-depth-min -100 @mod-depth-max 100 @mod-unit ms)
; pillow against the batter head
(param muffle @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; hole in the resonant head: vents the cavity, 1 = heads uncoupled
(param port @default 0.3 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param head_couple @default 0.6 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bottom_mix @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; shell knock (two inharmonic modes, fed by the beater force)
(param shell_pitch @default 180 @min 60 @max 1200 @unit Hz @mod true @mod-mode additive @mod-depth-min -400 @mod-depth-max 400 @mod-unit Hz)
(param shell_decay @default 120 @min 10 @max 800 @unit ms @mod true @mod-mode additive @mod-depth-min -200 @mod-depth-max 200 @mod-unit ms)
(param shell_level @default 0.3 @min 0 @max 3 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; beater contact transient (high-passed contact mic on the batter)
(param click @default 0.5 @min 0 @max 3 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; mic radiation tilt, weight_n *= ratio_n^bright (normalised, see below)
(param bright @default 0.4 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; ── output shaper (verbatim from modal-snare) ───────────────────────────────
(param drive @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param tone @default 0 @min -1 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param punch @default 0.3 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param level @default 0.18 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; ── bank + tone (verbatim from Drums/808 Kick; host-modulatable, top level) ─
(param bank @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_env @default 0.31 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_freq @default 0.03 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_res @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param lpf @default 18000 @min 200 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -9000 @mod-depth-max 9000 @mod-unit Hz)
(param hpf @default 20 @min 20 @max 500 @unit Hz @mod true @mod-mode additive @mod-depth-min -240 @mod-depth-max 240 @mod-unit Hz)
; shared one-pole time constant for the bank/tone modulatable params (808)
(param smoothing @default 5 @min 0 @max 100 @unit ms)

(def release-v (clip (mod release) 20 3000))
(def release2-v (clip (mod release2) 20 3000))
(def tune-v (clip (mod tune) -60 12))
(def pitch2-ratio-v (clip (mod pitch2_ratio) 0.5 2))
(def stretch-v (clip (mod stretch) 0.4 1.6))
(def tilt-v (clip (mod tilt) 0 2.5))
(def beater-hard-v (clip (mod beater_hard) 0.00002 0.01))
(def beater-speed-v (clip (mod beater_speed) 0.002 0.2))
(def beater-size-v (clip (mod beater_size) 0.03 0.45))
(def bend-v (clip (mod bend) 0 4))
(def bend-time-v (clip (mod bend_time) 5 400))
(def muffle-v (clip (mod muffle) 0 1))
(def port-v (clip (mod port) 0 1))
(def head-couple-v (clip (mod head_couple) 0 2))
(def bottom-mix-v (clip (mod bottom_mix) 0 2))
(def shell-pitch-v (clip (mod shell_pitch) 60 1200))
(def shell-decay-v (clip (mod shell_decay) 10 800))
(def shell-level-v (clip (mod shell_level) 0 3))
(def click-v (clip (mod click) 0 3))
(def bright-v (clip (mod bright) 0 2.5))
(def drive-v (clip (mod drive) 0 1))
(def tone-v (clip (mod tone) -1 1))
(def punch-v (clip (mod punch) 0 1))
(def level-v (clip (mod level) 0 2))

; ── Mode tables (baked; see header) ─────────────────────────────────────────
; modes (m,n): [(0,1) (1,1) (2,1) (0,2) (3,1) (1,2) (4,1) (2,2) (0,3) (5,1) (3,2) (6,1) (1,3) (4,2) (7,1) (2,3)]
; k_n = 2.4048 * ratio_n (unit radius)
(def k-ratio (tensor @shape [4 4] @data [
  1.0000 1.5933 2.1355 2.2954
  2.6531 2.9173 3.1555 3.5001
  3.5985 3.6475 4.0589 4.1317
  4.2304 4.6010 4.6101 4.8319
]))
; beater at r=0.12, psi/modal-mass, max-normalised, no tip lowpass baked
(def k-strike (tensor @shape [4 4] @data [
  0.2250 0.1709 0.0494 0.4777
  0.0125 0.5289 0.0030 0.1968
  0.6285 0.0007 0.0611 0.0001
  1.0000 0.0171 0.0000 0.4721
]))
(def k-mic-bat (tensor @shape [4 4] @data [
  0.7280 0.5791 0.4159 -0.0404
  0.2858 0.2785 0.1914 0.4118
  -0.4020 0.1260 0.4304 0.0819
  -0.2512 0.3901 0.0528 -0.0314
]))
; |k-mic-bat| (baked): weights for the BRIGHT tilt normalisation (sum 4.6967)
(def k-mic-abs (tensor @shape [4 4] @data [
  0.7280 0.5791 0.4159 0.0404
  0.2858 0.2785 0.1914 0.4118
  0.4020 0.1260 0.4304 0.0819
  0.2512 0.3901 0.0528 0.0314
]))
(def k-mic-res (tensor @shape [4 4] @data [
  0.8306 0.5307 0.3055 0.2632
  0.1681 0.5078 0.0901 0.4849
  -0.2697 0.0474 0.3757 0.0246
  0.1119 0.2619 0.0127 0.3471
]))
; asymmetric-mode selector: the (0,n) entries are the scalar proxies below
(def k-asym (tensor @shape [4 4] @data [
  0 1 1 0
  1 1 1 1
  0 1 1 1
  1 1 1 1
]))

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history bat1 @shape [4 4])   ; batter asymmetric modes (t)
(make-tensor-history bat2 @shape [4 4])   ; (t-1)
(make-tensor-history res1 @shape [4 4])   ; reso asymmetric modes (t)
(make-tensor-history res2 @shape [4 4])
(make-history beatx)
(make-history beatv)
(make-history bendenv)
(make-history sh1y1) (make-history sh1y2)
(make-history sh2y1) (make-history sh2y2)
(make-history dcx1) (make-history dcy1)
(make-history tonelp) (make-history punchenv)
(make-history ckl1) (make-history ckl2)
; (0,n) proxies: batter bp*, reso rp*
(make-history volbh) (make-history volrh)
(make-history bp1a) (make-history bp1b)
(make-history bp2a) (make-history bp2b)
(make-history bp3a) (make-history bp3b)
(make-history rp1a) (make-history rp1b)
(make-history rp2a) (make-history rp2b)
(make-history rp3a) (make-history rp3b)

; ── Read state (each history read EXACTLY once) ─────────────────────────────
(def bat1v (read-tensor-history bat1))
(def bat2v (read-tensor-history bat2))
(def res1v (read-tensor-history res1))
(def res2v (read-tensor-history res2))
(def bp1y1 (read-history bp1a)) (def bp1y2 (read-history bp1b))
(def bp2y1 (read-history bp2a)) (def bp2y2 (read-history bp2b))
(def bp3y1 (read-history bp3a)) (def bp3y2 (read-history bp3b))
(def rp1y1 (read-history rp1a)) (def rp1y2 (read-history rp1b))
(def rp2y1 (read-history rp2a)) (def rp2y2 (read-history rp2b))
(def rp3y1 (read-history rp3a)) (def rp3y2 (read-history rp3b))

; ── Beater: lumped mass in Hertz contact (the snare's striker, felt-soft) ───
; Contact time falls out of hardness and speed: at the defaults the mass
; stays in contact ~3 ms; `beater_hard` 0.004 (a stick) is ~0.5 ms.
(def beat-x (gswitch trigger 0.0001 (read-history beatx)))
(def beat-v (gswitch trigger (* -1 (* beater-speed-v vel-gain))
                     (read-history beatv)))
(def beat-pen (max (* beat-x -1) 0))
(def beat-f (min (* beater-hard-v beat-pen (sqrt beat-pen)) 0.01))
(def beat-v-next (+ beat-v beat-f))
(def beat-x-next (+ beat-x beat-v-next))
(write-history beatx beat-x-next)
(write-history beatv beat-v-next)
; SPEED loudness compensation (measured on the snare: RMS ~linear in speed),
; so SPEED sets the attack character and velocity keeps the dynamics.
(def speed-comp (/ 0.03 (max beater-speed-v 0.002)))
(def strike-force (* beat-f -1 0.3 speed-comp))

; peak-held beater force drives the pitch drop; settle time from bend_time
(def bend-coef (exp (/ -1000 (* samplerate bend-time-v))))
; beat-f peaks ~7e-4 at the felt defaults: 800 puts benv near 0.6, so
; bend=1 starts the hit ~25% sharp and settles over bend_time
(def benv (max (* (read-history bendenv) bend-coef) (* beat-f 800)))
(write-history bendenv benv)

; ── Mode frequencies / per-mode decay ───────────────────────────────────────
(def f0 (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 20 1000))
(def bat-r (max (+ 1 (* (- k-ratio 1) stretch-v)) 0.3))
(def r2s (+ 1 (* 1.2954 stretch-v)))
(def r3s (+ 1 (* 2.5985 stretch-v)))
; tension x(1+bend*benv) is pitch x sqrt(...)
(def bend-mul (sqrt (+ 1 (* bend-v benv))))
(def f0-res (clip (* f0 pitch2-ratio-v) 20 1000))
(def f-bat (max (* bat-r f0 bend-mul) 20))
(def f-res (max (* bat-r f0-res) 20))
(def w-bat (min (* twopi (/ f-bat samplerate)) 2.83))
(def w-res (min (* twopi (/ f-res samplerate)) 2.83))
(def t60-bat (max (* release-v 0.001 (pow bat-r (* tilt-v -1))) 0.002))
(def t60-res (max (* release2-v 0.001 (pow bat-r (* tilt-v -1))) 0.002))
(def r-bat (exp (/ -6.9077553 (* samplerate t60-bat))))
(def r-res (exp (/ -6.9077553 (* samplerate t60-res))))
; muffle = per-sample state shrink (dissipative by construction). A pillow
; covers a big patch of the head, so every mode feels it, the upper ones
; more (sqrt ratio). Full muffle puts the fundamental near 15 ms e-fold.
(def muffle-eff (* muffle-v muffle-v 0.0015))
(def dm-bat (max (- 1 (* muffle-eff (sqrt bat-r))) 0.75))
(def bat1d (* bat1v dm-bat))
(def bat2d (* bat2v dm-bat))

; beater spread over the modes: exp(-(k_n * size)^2), k_n = 2.405 * ratio_n
(defmacro sizespread (rat)
  (exp (* -1 (* 2.405 rat beater-size-v) (* 2.405 rat beater-size-v))))
(def spread (sizespread bat-r))
(def spread1 (sizespread 1))
(def spread2 (sizespread r2s))
(def spread3 (sizespread r3s))
(def strike-mask-m (* k-strike spread))

; ── Shell: 2 inharmonic modal resonators (the snare's rimmode) ──────────────
(defmacro shellmode (x f t60s y1h y2h)
  (def w0 (* twopi (/ (clip f 40 18000) samplerate)))
  (def rr (exp (/ -6.9077553 (* samplerate (max t60s 0.001)))))
  (def y1v (read-history y1h))
  (def y2v (read-history y2h))
  (def y (+ (* 2 rr (cos w0) y1v) (* -1 rr rr y2v) x))
  (write-history y2h y1v)
  (write-history y1h y)
  y)
(def shell-t60 (* shell-decay-v 0.001))
(def shell-in (* strike-force 400))
(def shell-m1 (shellmode shell-in shell-pitch-v shell-t60 sh1y1 sh1y2))
(def shell-m2 (shellmode shell-in (* shell-pitch-v 1.62) (* shell-t60 0.6) sh2y1 sh2y2))
(def shell-sig (+ shell-m1 (* shell-m2 0.5)))

; ── (0,n) proxies: scalar volume modes of both heads, two-way air coupling ──
(defmacro pm2 (px pw0 prr pdmv py1v py2v py1h py2h)
  (def py1d (* py1v pdmv))
  (def py (+ (* 2 prr (cos pw0) py1d) (* -1 prr prr (* py2v pdmv)) px))
  (write-history py2h py1d)
  (write-history py1h py)
  py)
(def wb1 (min (* twopi (/ (* f0 bend-mul) samplerate)) 2.83))
(def wb2 (min (* twopi (/ (* f0 bend-mul r2s) samplerate)) 2.83))
(def wb3 (min (* twopi (/ (* f0 bend-mul r3s) samplerate)) 2.83))
(def wr1 (min (* twopi (/ f0-res samplerate)) 2.83))
(def wr2 (min (* twopi (/ (* f0-res r2s) samplerate)) 2.83))
(def wr3 (min (* twopi (/ (* f0-res r3s) samplerate)) 2.83))
(def rb1 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001) 0.002)))))
(def rb2 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 (pow r2s (* tilt-v -1))) 0.002)))))
(def rb3 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 (pow r3s (* tilt-v -1))) 0.002)))))
(def rr1 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001) 0.002)))))
(def rr2 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 (pow r2s (* tilt-v -1))) 0.002)))))
(def rr3 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 (pow r3s (* tilt-v -1))) 0.002)))))
(def dmb1 (- 1 muffle-eff))
(def dmb2 (- 1 (* muffle-eff (sqrt r2s))))
(def dmb3 (- 1 (* muffle-eff (sqrt r3s))))
; air column: relative spring between the two heads' volume displacements
; from LAST outputs. kc/k_mode = 1.27*head_couple at the fundamental (the
; snare's grid-matched law); a port hole vents it.
; volume sums come through their own histories (one extra sample of delay,
; nothing at 55 Hz) so the coupling force is a plain history read.
(def vol-b (read-history volbh))
(def vol-r (read-history volrh))
(def kc (* head-couple-v (- 1 port-v) 1.27 wr1 wr1))
(def air-b (* kc (- vol-r vol-b)))
(def air-r (* kc (- vol-b vol-r)))
; batter proxies: beater ((0,n) strike weights x spread) + air
(def sb1 (+ (* strike-force spread1 0.2250) air-b))
(def sb2 (+ (* strike-force spread2 0.4777) (* air-b -0.2855)))
(def sb3 (+ (* strike-force spread3 0.6285) (* air-b 0.1453)))
(def bq1 (pm2 sb1 wb1 rb1 dmb1 bp1y1 bp1y2 bp1a bp1b))
(def bq2 (pm2 sb2 wb2 rb2 dmb2 bp2y1 bp2y2 bp2a bp2b))
(def bq3 (pm2 sb3 wb3 rb3 dmb3 bp3y1 bp3y2 bp3a bp3b))
; reso proxies: air + a little shell bleed of the strike
(def sr1 (+ air-r (* strike-force 0.2250 0.03)))
(def sr2 (+ (* air-r -0.2855) (* strike-force 0.4777 0.03)))
(def sr3 (+ (* air-r 0.1453) (* strike-force 0.6285 0.03)))
(def zp1 (pm2 sr1 wr1 rr1 1 rp1y1 rp1y2 rp1a rp1b))
(def zp2 (pm2 sr2 wr2 rr2 1 rp2y1 rp2y2 rp2a rp2b))
(def zp3 (pm2 sr3 wr3 rr3 1 rp3y1 rp3y2 rp3a rp3b))
(write-history volbh (+ bq1 (* -0.2855 bq2) (* 0.1453 bq3)))
(write-history volrh (+ zp1 (* -0.2855 zp2) (* 0.1453 zp3)))

; ── Bank updates (asymmetric modes only; y = 2r cos(w) y1 - r² y2 + x) ──────
; TWO-WAY per-mode head coupling, elementwise from last-sample states.
; COMPILER GOTCHA (measured, scratchpad C diff): when the tensor coupling
; shares the `kc` node with the scalar proxy cycle, DGenLisp fuses the whole
; scalar feedback cluster INSIDE the 16-element tensor loop, so every proxy
; 2-pole runs 16x per sample (fundamental at 16x pitch, decay 16x shorter).
; A separate, not-CSE-able node for the tensor path keeps the clusters apart.
(def kct (* head-couple-v (- 1 port-v) 1.27 wr1 wr1 1.0000001))
(def couple-b (* kct (- res1v bat1v)))
(def couple-r (* kct (- bat1v res1v)))
(def x-bat (* (+ (* strike-force strike-mask-m) couple-b) k-asym))
(def bat-next (+ (* 2 r-bat (cos w-bat) bat1d) (* -1 r-bat r-bat bat2d) x-bat))
(def bat-nextc (max (min bat-next 3) -3))
(def x-res (* (+ (* strike-force strike-mask-m 0.03) couple-r) k-asym))
(def res-next (+ (* 2 r-res (cos w-res) res1v) (* -1 r-res r-res res2v) x-res))
(def res-nextc (max (min res-next 3) -3))

; ── Output taps ─────────────────────────────────────────────────────────────
; BRIGHT is a normalised spectral tilt (see modal-snare): the mic's summed
; sensitivity is held constant while the balance moves to the overtones.
(def bright-w (pow bat-r bright-v))
(def bright-norm (pow (/ 4.6967 (max (sum (* k-mic-abs bright-w)) 0.001)) 0.15))
(def mic-top (* 14 bright-norm (+ (sum (* bat-nextc k-mic-bat bright-w))
                      (* bq1 0.7280)
                      (* bq2 -0.0404 (pow r2s bright-v))
                      (* bq3 -0.4020 (pow r3s bright-v)))))
(def mic-bot (* 14 bright-norm (+ (sum (* res-nextc k-mic-res bright-w))
                      (* zp1 0.8306)
                      (* zp2 0.2632 (pow r2s bright-v))
                      (* zp3 -0.2697 (pow r3s bright-v)))))
; click: the beater's contact slap. A felt pulse is ~3 ms long, so high-
; passing the force itself leaves nothing above 1 kHz (measured: click 0 and
; click 3 were identical). The slap is broadband noise gated by the RISING
; edge of the contact force: a wood beater's shorter rise gives a tighter
; tick, and the burst scales with the impact.
(defmacro hp1 (x hist)
  (def lpv (read-history hist))
  (def lpn (+ lpv (* 0.1 (- x lpv))))
  (write-history hist lpn)
  (- x lpn))
(def sf-prev (read-history ckl2))
(write-history ckl2 strike-force)
(def sf-onset (max (- sf-prev strike-force) 0))
(def click-hp (hp1 (* (noise) sf-onset) ckl1))
; the onset ramp is ~4e-6 per sample against a head mic near 4: 1e6 puts
; click=1 at about half the head peak for ~1.5 ms
(def mic-click (* click-hp 1000000))
(def mixdown (+ mic-top (* mic-bot bottom-mix-v) (* mic-click click-v)
                (* shell-sig shell-level-v 0.1)))

; ── Write feedback ──────────────────────────────────────────────────────────
(write-tensor-history bat2 bat1d)
(write-tensor-history bat1 bat-nextc)
(write-tensor-history res2 res1v)
(write-tensor-history res1 res-nextc)

; ── Shaper: tone tilt -> punch -> drive -> DC block (verbatim) ──────────────
(def xs (* mixdown 0.25))
(def tlp-prev (read-history tonelp))
(def tlp (+ tlp-prev (* 0.15 (- xs tlp-prev))))
(write-history tonelp tlp)
(def toned (+ (* tlp (- 1 (* (max tone-v 0) 0.85)))
              (* (- xs tlp) (+ 1 tone-v))))
(def penv (max (* (read-history punchenv) 0.9985) trigger))
(write-history punchenv penv)
(def punched (* toned (+ 1 (* punch-v 3 penv))))
(def pre (+ 1 (* drive-v 12)))
(def driven (/ (tanh (* punched pre)) (tanh pre)))
(def bodied (* driven 4))
(def dcy (+ (- bodied (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 bodied)
(write-history dcy1 dcy)
(def shaped-out (* dcy level-v vel-gain))

; ── BANK + TONE post chain (from Drums/808 Kick / Drums/909 Kick) ───────────
; The modal shaper (tone tilt -> punch -> drive -> DC block -> level) is
; the "voice"; the bank and tone pair sit after it exactly as they sit
; after the 808/909 voices. The four host-modulatable bank params and the
; tone pair are one-pole smoothed by `smoothing` exactly like the 808.

; History-based one-pole parameter smoother (from drums/synthid-808). Each
; expansion owns independent value and initialization history. The first
; sample adopts the current value directly, avoiding a startup ramp away
; from the identified defaults.
(defmacro onepole-param (input time_ms)
  (make-history value_h)
  (make-history initialized_h)
  (def previous (read-history value_h))
  (def initialized (read-history initialized_h))
  (def safe_seconds (* (max time_ms 0.001) 0.001))
  (def coefficient (exp (/ -1.0 (* samplerate safe_seconds))))
  (def filtered (+ (* (- 1.0 coefficient) input) (* coefficient previous)))
  (def initialized_value (gswitch (lt initialized 0.5) input filtered))
  (def output (gswitch (lt time_ms 0.001) input initialized_value))
  (write-history value_h output)
  (write-history initialized_h 1.0)
  output)

; Resettable exponential decay envelope (T60 in ms), value 1.0 on the
; trigger sample.
(defmacro id-env (trig decay_ms)
  (make-history e_h)
  (def coef (exp (/ -6.9077553 (max 1.0 (* decay_ms 0.001 samplerate)))))
  (def next (gswitch (gt trig 0.5) 1.0 (* (read-history e_h) coef)))
  (write-history e_h next)
  next)

; ======================================================================
; BANK: the sc-filterbank core (Sherman FB2, content/effects/sc-filterbank)
; with its cutoff riding a per-trigger decay envelope — the "trigger a hit
; and ride freq from 0.34 down to 0.03" gesture as two knobs. Serial
; F1(LP) -> F2(~BP through the /4 clock divider), van der Pol resonance,
; VCO slew + charge-injection thump, shared compressing output tanh.
; Exact bypass at bank 0 (the cores keep running so engaging is click-free).
; ======================================================================

; One switched-cap SVF core: input sampled on the tick, Chamberlin update
; gated to the tick, states held (ZOH) between ticks. Biased tanh on the
; bp state injection-locks the scream; amplitude-dependent damping (van
; der Pol) gives a hard self-osc threshold. (From sc-filterbank.)
(defmacro bank-svf (sig tick morph gcoef kbase)
  (make-history lp_h)
  (make-history bp_h)
  (def bk_xs (latch sig tick))
  (def keff (+ kbase (* 1.2 (* (read-history bp_h) (read-history bp_h)))))
  (def hp (- bk_xs (+ (read-history lp_h) (* keff (read-history bp_h)))))
  (def bpn (* 1.078 (- (tanh (+ (+ (read-history bp_h) (* gcoef hp)) 0.28)) (tanh 0.28))))
  (def lpn (+ (read-history lp_h) (* gcoef bpn)))
  (write-history bp_h (mix (read-history bp_h) bpn tick))
  (write-history lp_h (mix (read-history lp_h) lpn tick))
  (def lpw (clip (- 1 (* 2 morph)) 0 1))
  (def hpw (clip (- (* 2 morph) 1) 0 1))
  (+ (* (read-history lp_h) lpw)
     (+ (* (read-history bp_h) (- 1 (+ lpw hpw)))
        (* hp hpw))))

(defmacro bank-stage (sig triggered wet_a env_a freq_a res_a note_in)
  ; Defaults are the exact settings the gesture was discovered with:
  ; freq 0.34 -> 0.03 (floor 0.03 + env 0.31), res 0.75, mode1 0.00,
  ; mode2 0.51, harm 5, crunch 0.00, ser 1.00, blend 0.50, drive 0.81.
  ; bank_freq (FLR) and bank_res (RES) are top-level @mod params, passed
  ; in as freq_a / res_a.
  (param bank_time @default 260 @min 20 @max 2000 @unit ms)
  (param bank_harm @default 5 @min 0 @max 7)
  (param bank_crunch @default 0 @min 0 @max 1)
  (param bank_drive @default 0.81 @min 0 @max 1)
  ; Keytrack MODE (default key): 1 shifts the whole sweep (floor, start,
  ; both resonances, and the clock — so the ZOH/aliasing artifacts too)
  ; with the note, in the log-cutoff domain, relative to the
  ; default 55 Hz fundamental. At that reference pitch the two modes are
  ; identical — so the discovered sound is unchanged there. 0 = free
  ; (fixed frequencies). Follows tune and glide; intermediates blend.
  (param bank_track @default 1 @min 0 @max 1)
  ; Reconstruction filter (the thing after the chip that Sherman barely
  ; has): a one-pole tracking the CLOCK at 0.35*fclk — above the passband
  ; (cutoff = fclk/ratio, tone untouched) but below the ZOH image bands,
  ; so it eats the staircase aliasing wherever the sweep sits. 0 = raw
  ; hardware grit, 1 = fully reconstructed (default).
  (param bank_recon @default 1 @min 0 @max 1)

  (def wet_amt (clip wet_a 0 1))
  ; note_in is the batter fundamental f0 (host pitch x tune, before the
  ; bend), continuous, so tune modulation retracks the sweep. Reference =
  ; 55 Hz, the default (A4 host pitch, tune -36): at that pitch key and
  ; free are identical.
  (def bk_note (max note_in 1.0))
  (def bk_key_off (* (clip bank_track 0 1) (/ (log (/ bk_note 55.0)) 5.586)))
  ; input drive: the builtin Filterbank's drive circuit
  ; (effects/filterbank.rs §2) — dynamic-bias coupling-cap sag, +6 dB
  ; pre-emphasis @ 3 kHz, 0.55·tube + 0.45·diode asymmetric shaper (roar
  ; transfer bank), matched de-emphasis, 10 Hz DC blocker. The builtin's
  ; 4x oversampling is deliberately omitted: this bank aliases by design,
  ; and bank_recon is the cleanup control.
  (def gained (* sig (+ 1 (* bank_drive 24))))

  ; dynamic bias — a 2 ms / 80 ms follower
  ; of the driven signal shifts the operating point into the asymmetric
  ; curve, so transients bloom and sustained material sits down
  (make-history bk_biash)
  (def bmag (abs gained))
  (def bprev (read-history bk_biash))
  (def bcoef (gswitch (gt bmag bprev)
                      (- 1.0 (exp (/ -1.0 (* 0.002 samplerate))))
                      (- 1.0 (exp (/ -1.0 (* 0.080 samplerate))))))
  (def bk_benv (+ bprev (* bcoef (- bmag bprev))))
  (write-history bk_biash bk_benv)
  (def dbias (* 0.22 (tanh bk_benv)))
  ; pre-emphasis: +6 dB above 3 kHz so the highs clip first
  (def ecoef (- 1.0 (exp (/ (* -2.0 pi 3000.0) samplerate))))
  (make-history bk_emph)
  (def emph_lp (+ (read-history bk_emph) (* ecoef (- gained (read-history bk_emph)))))
  (write-history bk_emph emph_lp)
  (def sh_in (+ gained (- gained emph_lp) dbias))
  ; 0.55 tube + 0.45 diode, unity small-signal slope (roar transfer bank)
  (def tube_u (max sh_in -2.4))
  (def sh_tube (tanh (+ tube_u (* 0.2 tube_u tube_u))))
  ; exp argument clamped at 0 so the unselected branch stays finite for
  ; negative inputs (gswitch evaluates both sides)
  (def dpos (gswitch (lt sh_in 0.35)
                     sh_in
                     (+ 0.35 (/ (- 1.0 (exp (* -3.0 (max (- sh_in 0.35) 0.0)))) 3.0))))
  (def sh_diode (gswitch (gte sh_in 0.0) dpos (* 1.2 (tanh (/ sh_in 1.2)))))
  (def shaped_drv (+ (* 0.55 sh_tube) (* 0.45 sh_diode)))
  ; matched de-emphasis (product ~ flat when clean), then 10 Hz DC block
  ; (the asymmetric curve + bias ride on an offset)
  (make-history bk_deemph)
  (def deemph_lp (+ (read-history bk_deemph) (* ecoef (- shaped_drv (read-history bk_deemph)))))
  (write-history bk_deemph deemph_lp)
  (def de_drv (- shaped_drv (* 0.5 (- shaped_drv deemph_lp))))
  (def dc_r (exp (/ (* -2.0 pi 10.0) samplerate)))
  (make-history bk_dcx)
  (make-history bk_dcy)
  (def bk_dcy (+ (- de_drv (read-history bk_dcx)) (* dc_r (read-history bk_dcy))))
  (write-history bk_dcx de_drv)
  (write-history bk_dcy bk_dcy)
  (def x bk_dcy)
  ; input envelope (charge-injection bleed keying), ~10 ms follower
  (make-history bk_envh)
  (def bk_env (+ (read-history bk_envh) (* 0.003 (- (abs x) (read-history bk_envh)))))
  (write-history bk_envh bk_env)

  ; cutoff position: floor + per-trigger decay sweep (replaces the LFO)
  (def sweep_env (id-env triggered bank_time))
  (def fpos_target (clip (+ (clip freq_a 0 1) bk_key_off (* (clip env_a 0 1) sweep_env)) 0 1))
  ; VCO slew: the expo converter lags, asymmetrically (up faster than down)
  (make-history bk_fposh)
  (def fpos_diff (- fpos_target (read-history bk_fposh)))
  (def fpos (+ (read-history bk_fposh)
               (* (mix 0.0015 0.006 (> fpos_diff 0)) fpos_diff)))
  (write-history bk_fposh fpos)
  (def fc (* 30 (exp (* 5.586 fpos))))

  ; switched-cap clock: crunch morphs ratio 100:1 -> 25:1 (log)
  (def ratio (* 100 (exp (* bank_crunch (log 0.25)))))
  (def gcoef (* 2 (sin (/ pi ratio))))
  (def kbase (- (* 2.08 (- 1 (clip res_a 0 1))) 0.22))

  ; clock jitter, depth keyed to crunch
  (make-history bk_nzh)
  (def bk_nz (+ (read-history bk_nzh) (* 0.05 (- (noise) (read-history bk_nzh)))))
  (write-history bk_nzh bk_nz)
  (def fclk (clip (* (* fc ratio) (+ 1 (* (* 0.012 (+ 0.3 bank_crunch)) bk_nz)))
                  200 (* samplerate 0.99)))
  (def ph1 (phasor fclk))
  ; explicit wrap detector: ramp2trig misses wraps near the host rate
  (make-history bk_prevph)
  (def tick1 (< ph1 (read-history bk_prevph)))
  (write-history bk_prevph ph1)

  ; clock divider: F2's clock is F1's through the selected ratio
  ; (selector is 1-based; floor needs dgenlisp >= v0.1.6). The knob moves
  ; in 0.5 steps: halves land midway between adjacent tap ratios, which
  ; the subtract-N accumulator divides as happily as the named taps.
  (def harm_q (/ (round (* (clip bank_harm 0 7) 2)) 2))
  (def harm_i (floor harm_q))
  (def harm_f (- harm_q harm_i))
  (def div_a (selector (+ 1 harm_i) 1 1.2 1.5 2 3 4 5 7))
  (def div_b (selector (+ 1 (clip (+ harm_i 1) 0 7)) 1 1.2 1.5 2 3 4 5 7))
  (def divisor (mix div_a div_b harm_f))
  (make-history bk_divcnt)
  (def cnt (+ (read-history bk_divcnt) tick1))
  (def fire2 (>= cnt divisor))
  (write-history bk_divcnt (- cnt (* divisor fire2)))
  (def tick2 (* tick1 fire2))

  ; sweep thump: charge injection puts a moving DC offset into the loop
  (def thump (* 60 fpos_diff (mix 0.0015 0.006 (> fpos_diff 0))))
  (def xin (+ x thump))

  (def f1 (bank-svf xin tick1 0.0 gcoef kbase))
  ; serial: F1's resonance overdrives the stage feeding F2
  (def f2in (tanh (* 1.7 f1)))
  (def f2 (bank-svf f2in tick2 0.51 gcoef kbase))

  ; clock bleed as charge injection, rising as the clock falls audible.
  ; Deviation from the effect port: the hardware's constant 0.3 idle-bleed
  ; floor is removed — an instrument must go silent between hits, so the
  ; bleed is keyed entirely to the input envelope (1.9 keeps the same
  ; peak level the effect has at full program).
  (def bleed (* (* (* (* bank_crunch bank_crunch)
                      (* 0.02 (clip (- 1 (/ fclk 6000)) 0 1)))
                   (* 1.9 bk_env))
                (- (* 2 (< ph1 0.5)) 1)))

  ; shared output stage: envelope-coupled gain into ONE tanh (the scream
  ; eats headroom and the program ducks under it)
  (def bk_pre (+ (* 0.5 (+ f1 f2)) bleed))
  (make-history bk_cmph)
  (def cmpa (abs bk_pre))
  (write-history bk_cmph (+ (read-history bk_cmph)
                            (* (mix 0.0004 0.02 (> cmpa (read-history bk_cmph)))
                               (- cmpa (read-history bk_cmph)))))
  (def cmp (/ 1 (+ 1 (* 3.2 (read-history bk_cmph)))))
  (def wet (* 0.85 (tanh (* 1.7 (* bk_pre cmp)))))

  ; clock-tracking reconstruction filter: two cascaded one-poles
  ; (12 dB/oct) at 0.35*fclk (see bank_recon above)
  (def rc_cut (clip (* fclk 0.35) 60 18000))
  (def rc_coef (exp (/ (* -2.0 pi rc_cut) samplerate)))
  (make-history bk_rch1)
  (def rc_1 (+ (* (- 1.0 rc_coef) wet) (* rc_coef (read-history bk_rch1))))
  (write-history bk_rch1 rc_1)
  (make-history bk_rch2)
  (def rc_2 (+ (* (- 1.0 rc_coef) rc_1) (* rc_coef (read-history bk_rch2))))
  (write-history bk_rch2 rc_2)
  (def wet_recon (mix wet rc_2 (clip bank_recon 0 1)))
  (mix sig wet_recon wet_amt))

; Final HP/LP tone pair, AFTER the bank so its screams, bleed and
; aliasing can be tamed. The filters always run (state stays warm) and are
; crossfaded out near the identity extremes: blend 0 at the defaults
; (exact dry pass-through), full filter by 35 Hz / 16000 Hz. A p-lock
; sweeping across the extreme therefore never hard-swaps signals.
(defmacro tone-stage (x lpf_hz hpf_hz)
  (def hp_filtered (svf x (clip hpf_hz 20 500) 0.707 2))
  (def hp_blend (clip (* (- hpf_hz 20.0) 0.0667) 0 1))
  (def hp_out (mix x hp_filtered hp_blend))
  (def lp_filtered (svf hp_out (clip lpf_hz 200 18000) 0.707 0))
  (def lp_blend (clip (* (- 17500.0 lpf_hz) 0.000667) 0 1))
  (def lp_out (mix hp_out lp_filtered lp_blend))
  lp_out)


(def bank_s (onepole-param (mod bank) smoothing))
(def bank_env_s (onepole-param (mod bank_env) smoothing))
(def bank_freq_s (onepole-param (mod bank_freq) smoothing))
(def bank_res_s (onepole-param (mod bank_res) smoothing))
(def lpf_s (onepole-param (mod lpf) smoothing))
(def hpf_s (onepole-param (mod hpf) smoothing))

(def banked (bank-stage shaped-out trigger bank_s bank_env_s bank_freq_s bank_res_s f0))
(def toned-out (tone-stage banked lpf_s hpf_s))
(out toned-out 1 @name audio)
