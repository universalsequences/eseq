; Modal Snare Wired — fork of drums/modal-snare that chases the three things
; the 6x6 FDTD membrane-snare-rim has and the first modal port did not:
;
;   1. A CHAOTIC WIRE BED. The grid has 36 wire/head contact points on a
;      surface that carries every reso-head mode; modal-snare bounced 12
;      whole-wire 2-poles off ONE scalar (the three (0,n) proxies), so every
;      wire landed once per cycle and the rattle was a periodic pulse train at
;      the wire pitch (a guiro, not a snare). Here the wires sense the reso
;      head at 4 contact zones along a diameter (baked readout tables, m>0
;      modes included), carry two inharmonic partials, and — the part that
;      actually matters — are PROJECTED onto the head surface on contact
;      like the grid's wires, so they never phase-lock. The
;      reso BANK never receives anything from the wires (the compiler kills a
;      bank fed by its own reduce, see modal-snare); only the scalar proxies
;      take the contact pseudo-loss.
;   2. SPLIT DEGENERATE PAIRS. Every m>0 membrane mode is a degenerate cos/sin
;      pair; a real (anisotropic) head splits them and beats. The grid got
;      that density for free (square membrane, both orientations). Banks are
;      [12 6]: rows 0-5 the cos set, 6-11 the sin partners, detuned by `split`
;      (percent, per-mode baked sign/magnitude). The strike sits at 20 deg
;      to the anisotropy axis so both orientations are excited.
;   3. VISCOUS DECAY LAW. The grid's tone_damp is a Laplacian viscosity, so
;      T60_n ~ 1/(1+r^2): partials above 3x the fundamental are gone inside
;      25 ms and the body is a dark thump. `visc` reproduces that law
;      (0.5 = the grid's exact ratio); `tilt` stays as the old power law.
;
; Everything else (Hertz striker, stroke/press, rim hoop, (0,n) scalar
; proxies with two-way air coupling, elementwise two-way bank coupling,
; shaper) is modal-snare's, and its gain staging is kept: the (0,1) open
; strike weight is pinned to the same 0.2855 so the proxy constants carry.
; Tables are baked by gen_tables.py next to this file (numpy only); re-run
; it after changing any geometry constant — it rewrites the block below.

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

; ── Params (names match modal-snare where the meaning is the same) ─────────
(param release @default 180 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)      ; batter fundamental T60
(param release2 @default 260 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)     ; reso fundamental T60
(param tune @default -14.5 @min -48 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch2_ratio @default 1.35 @min 0.25 @max 4 @mod true @mod-mode additive @mod-depth-min -1.5 @mod-depth-max 1.5)
(param bend @default 0.5 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
; partial spread: 1 = ideal Bessel ratios, <1 air-loaded, >1 plate-like
(param stretch @default 0.9 @min 0.4 @max 1.6 @mod true @mod-mode additive @mod-depth-min -0.5 @mod-depth-max 0.5)
; per-mode decay power law, T60_n = release * ratio_n^-tilt (0 = off)
(param tilt @default 0 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; NEW: viscous decay law, T60_n = release / (1 + visc * (ratio_n^2 - 1)).
; 0.5 is exactly the grid's tone_damp law at its default; 0 = off.
(param visc @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; NEW: degenerate-pair split in percent. 0 = both orientations exactly
; degenerate (modal-snare's spectrum, rotated); ~1 = a real head's beating.
(param split @default 1.0 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
; stick TIP radius (fraction of head radius): lowpasses the strike over modes
(param tip @default 0.2 @min 0.03 @max 0.45 @mod true @mod-mode additive @mod-depth-min -0.2 @mod-depth-max 0.2)
(param stick_hard @default 0.004 @min 0.0002 @max 0.05 @mod true @mod-mode additive @mod-depth-min -0.01 @mod-depth-max 0.01)
(param stick_speed @default 0.02 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)
(param scrape @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; stroke expression: 0 = ghost, 0.5 = open, 1 = rimshot; press = palm on head
(param stroke @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param press @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; metal hoop
(param rim_pitch @default 2200 @min 400 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param rim_decay @default 300 @min 20 @max 1500 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)
(param rim_level @default 1.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param rim_drive @default 0.001 @min 0 @max 0.005)
; batter <-> reso air coupling
(param head_couple @default 0.6 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; snare wires
(param wire_pitch @default 620 @min 100 @max 2400 @unit Hz @mod true @mod-mode additive @mod-depth-min -800 @mod-depth-max 800 @mod-unit Hz)
(param wire_decay @default 420 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit ms)
(param snare_tension @default 0.85 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; contact-mic gain per unit overlap
(param rattle @default 3 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
; energy the reso head's volume modes lose while wires touch (Sekiguchi
; pseudo-loss on the scalar proxies; the bank cannot take it, see header)
(param contact_loss @default 0.0005 @min 0 @max 0.02 @mod true @mod-mode additive @mod-depth-min -0.04 @mod-depth-max 0.04)
; NEW: level of each wire's second (stiff, 2.41x) partial. A coiled strand is
; not a pure string; the second partial intermodulates the bounce timing.
(param wire_tone @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param snares @default 0.6 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param bottom_mix @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; mic radiation tilt, weight_n *= ratio_n^bright
(param bright @default 0 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; contact restitution + 1: 1 = the wire stops on the surface, 2 = elastic
; (HARD MAX 2: above it the head Fermi-pumps the wires)
(param wire_kick @default 1.8 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; debug tap: 0 = audio, 1 = reso head at contact zone 2, 2 = wire 6, 3 = raw contact
(param dbg @default 0 @min 0 @max 3)
; palm damping strength (per-sample state shrink at full press)
(param palm @default 0.0015 @min 0.0005 @max 0.02)
; ── output shaper
(param drive @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param tone @default 0 @min -1 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param punch @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param level @default 0.25 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

(def release-v (clip (mod release) 20 3000))
(def release2-v (clip (mod release2) 20 3000))
(def tune-v (clip (mod tune) -48 24))
(def pitch2-ratio-v (clip (mod pitch2_ratio) 0.25 4))
(def bend-v (clip (mod bend) 0 4))
(def stretch-v (clip (mod stretch) 0.4 1.6))
(def tilt-v (clip (mod tilt) 0 2.5))
(def visc-v (clip (mod visc) 0 2))
(def split-v (clip (mod split) 0 4))
(def tip-v (clip (mod tip) 0.03 0.45))
(def stick-hard-v (clip (mod stick_hard) 0.0002 0.05))
(def stick-speed-v (clip (mod stick_speed) 0.002 0.2))
(def scrape-v (clip (mod scrape) 0 1))
(def stroke-v (clip (mod stroke) 0 1))
(def press-v (clip (mod press) 0 1))
(def rim-pitch-v (clip (mod rim_pitch) 400 8000))
(def rim-decay-v (clip (mod rim_decay) 20 1500))
(def rim-level-v (clip (mod rim_level) 0 4))
(def rim-drive-v (clip rim_drive 0 0.005))
(def head-couple-v (clip (mod head_couple) 0 2))
(def wire-pitch-v (clip (mod wire_pitch) 100 2400))
(def wire-decay-v (clip (mod wire_decay) 20 3000))
(def snare-tension-v (clip (mod snare_tension) 0 1))
(def rattle-v (clip (mod rattle) 0 8))
(def contact-loss-v (clip (mod contact_loss) 0 0.02))
(def wire-tone-v (clip (mod wire_tone) 0 1))
(def snares-v (clip (mod snares) 0 4))
(def bottom-mix-v (clip (mod bottom_mix) 0 2))
(def level-v (clip (mod level) 0 2))
(def drive-v (clip (mod drive) 0 1))
(def tone-v (clip (mod tone) -1 1))
(def punch-v (clip (mod punch) 0 1))
(def bright-v (clip (mod bright) 0 2.5))
(def wire-kick-v (clip (mod wire_kick) 0 2))

(def edge-mix (clip (* (- stroke-v 0.5) 2) 0 1))
(def ghost-mix (clip (* (- 0.5 stroke-v) 2) 0 1))

; ── Mode tables (baked by gen_tables.py; do not edit by hand) ───────────────
;; BEGIN GENERATED TABLES
; modes (m,n) in frequency order, rows 0-5 = cos set, 6-11 = sin partners:
; (0,1) (1,1) (2,1) (0,2) (3,1) (1,2) (4,1) (2,2) (0,3) (5,1) (3,2) (6,1) (1,3) (4,2) (7,1) (2,3) (0,4) (5,2) (3,3) (1,4) (6,2) (4,3) (2,4) (7,2) (0,5) (5,3) (3,4) (1,5) (6,3) (4,4) (2,5) (0,6) (7,3) (5,4) (3,5) (1,6)
; frequency ratio to the (0,1) fundamental
(def bat-ratio (tensor @shape [12 6] @data [
   1.0000  1.5933  2.1355  2.2954  2.6531  2.9173
   3.1555  3.5001  3.5985  3.6475  4.0589  4.1317
   4.2304  4.6010  4.6101  4.8319  4.9033  5.1308
   5.4121  5.5404  5.6508  5.9765  6.1526  6.1631
   6.2087  6.5286  6.7462  6.8490  7.0707  7.3253
   7.4682  7.5145  7.6045  7.8925  8.0710  8.1569
   1.0000  1.5933  2.1355  2.2954  2.6531  2.9173
   3.1555  3.5001  3.5985  3.6475  4.0589  4.1317
   4.2304  4.6010  4.6101  4.8319  4.9033  5.1308
   5.4121  5.5404  5.6508  5.9765  6.1526  6.1631
   6.2087  6.5286  6.7462  6.8490  7.0707  7.3253
   7.4682  7.5145  7.6045  7.8925  8.0710  8.1569
]))
; open strike r=0.4 th=20.0: psi/M * tip lowpass, scaled so (0,1)=0.2855
(def bat-open (tensor @shape [12 6] @data [
   0.2855  0.6210  0.4428  0.0824  0.2208  0.7161
   0.0539  0.7659 -0.3901 -0.0357  0.4876 -0.0650
  -0.1921  0.1419 -0.0608  0.2951 -0.2984 -0.1079
   0.3728 -0.7237 -0.2200  0.1492 -0.3711 -0.2262
   0.1509 -0.1392 -0.0326 -0.2367 -0.3300  0.0473
  -0.3859  0.2582 -0.3814 -0.0789 -0.2262  0.3060
   0.0000  0.2260  0.3715  0.0000  0.3825  0.2606
   0.3055  0.6427  0.0000  0.2023  0.8446  0.1126
  -0.0699  0.8047  0.0510  0.2476  0.0000  0.6117
   0.6458 -0.2634  0.3811  0.8463 -0.3114  0.1898
   0.0000  0.7897 -0.0564 -0.0861  0.5716  0.2681
  -0.3238  0.0000  0.3200  0.4475 -0.3918  0.1114
]))
; rim strike r=0.93 th=20.0
(def bat-edge (tensor @shape [12 6] @data [
   0.0329  0.1223  0.1509 -0.1034  0.1314 -0.2580
   0.0563 -0.2542  0.1669 -0.0659 -0.1887 -0.2128
   0.3534 -0.0713 -0.3541  0.3080 -0.1997  0.0748
   0.2067 -0.3805  0.2198  0.0715 -0.3035  0.3351
   0.1956 -0.0693 -0.1881  0.3434 -0.1889 -0.0605
   0.2543 -0.1633 -0.2683  0.0547  0.1471 -0.2671
   0.0000  0.0445  0.1266  0.0000  0.2275 -0.0939
   0.3194 -0.2133  0.0000  0.3735 -0.3268  0.3686
   0.1286 -0.4042  0.2971  0.2585  0.0000 -0.4245
   0.3580 -0.1385 -0.3807  0.4054 -0.2546 -0.2812
   0.0000  0.3930 -0.3258  0.1250  0.3272 -0.3430
   0.2134  0.0000  0.2251 -0.3103  0.2547 -0.0972
]))
; batter mic psi at r=0.55 th=60.0
(def bat-mic (tensor @shape [12 6] @data [
   0.6082  0.2838 -0.2396 -0.2720 -0.3879 -0.0054
  -0.1532 -0.0873 -0.2524  0.1190 -0.2887  0.1827
  -0.1674 -0.1732  0.0695  0.1496  0.2578  0.1818
   0.2017  0.0451  0.3541  0.0411  0.0381  0.1641
   0.1191  0.0168  0.1963  0.1190  0.1320  0.1281
  -0.1269 -0.2428  0.1034 -0.1302 -0.1913 -0.0700
   0.0000  0.4915  0.4151  0.0000  0.0000 -0.0093
  -0.2653  0.1512  0.0000 -0.2061  0.0000 -0.0000
  -0.2899 -0.3000  0.1203 -0.2592  0.0000 -0.3150
  -0.0000  0.0782 -0.0000  0.0712 -0.0660  0.2843
   0.0000 -0.0292 -0.0000  0.2061 -0.0000  0.2219
   0.2197  0.0000  0.1792  0.2255  0.0000 -0.1213
]))
; |bat-mic| for the BRIGHT normalisation
(def bat-mic-abs (tensor @shape [12 6] @data [
   0.6082  0.2838  0.2396  0.2720  0.3879  0.0054
   0.1532  0.0873  0.2524  0.1190  0.2887  0.1827
   0.1674  0.1732  0.0695  0.1496  0.2578  0.1818
   0.2017  0.0451  0.3541  0.0411  0.0381  0.1641
   0.1191  0.0168  0.1963  0.1190  0.1320  0.1281
   0.1269  0.2428  0.1034  0.1302  0.1913  0.0700
   0.0000  0.4915  0.4151  0.0000  0.0000  0.0093
   0.2653  0.1512  0.0000  0.2061  0.0000  0.0000
   0.2899  0.3000  0.1203  0.2592  0.0000  0.3150
   0.0000  0.0782  0.0000  0.0712  0.0660  0.2843
   0.0000  0.0292  0.0000  0.2061  0.0000  0.2219
   0.2197  0.0000  0.1792  0.2255  0.0000  0.1213
]))
; reso mic psi at r=0.5 th=152.0
(def res-mic (tensor @shape [12 6] @data [
   0.6699 -0.5127  0.2545 -0.1684 -0.0357 -0.1184
  -0.0936  0.1723 -0.3563  0.1377 -0.0401 -0.1250
   0.2967 -0.1482  0.0865 -0.1126  0.1208  0.2833
   0.0039  0.0942 -0.3184 -0.0407 -0.1394  0.2643
   0.2709  0.1670  0.0303 -0.2305 -0.2802  0.0934
   0.0786 -0.0990  0.3066 -0.1222  0.0015 -0.0803
   0.0000  0.2726 -0.3773  0.0000  0.3396  0.0630
  -0.2316 -0.2554  0.0000  0.1155  0.3819 -0.0266
  -0.1578 -0.3668 -0.0248  0.1669  0.0000  0.2377
  -0.0374 -0.0501 -0.0677 -0.1008  0.2067 -0.0758
   0.0000  0.1401 -0.2884  0.1225 -0.0596  0.2312
  -0.1165  0.0000 -0.0879 -0.1025 -0.0140  0.0427
]))
; palm energy fraction, gaussian bump at r=0.5 th=200.0 sigma=0.35, max-normalised
(def bat-palm (tensor @shape [12 6] @data [
   0.9180  1.0000  0.7340  0.8085  0.6692  0.9496
   0.6247  0.7824  0.8033  0.5887  0.7542  0.5592
   0.9436  0.7307  0.5346  0.7923  0.8012  0.7072
   0.7756  0.9416  0.6851  0.7623  0.7962  0.6645
   0.8002  0.7476  0.7842  0.9407  0.7327  0.7758
   0.7981  0.7997  0.7180  0.7659  0.7887  0.9402
   0.0000  0.6154  0.7237  0.0000  0.6712  0.6470
   0.6249  0.7747  0.0000  0.5887  0.7557  0.5592
   0.6530  0.7308  0.5346  0.7850  0.0000  0.7072
   0.7770  0.6553  0.6851  0.7624  0.7891  0.6645
   0.0000  0.7476  0.7856  0.6563  0.7327  0.7759
   0.7911  0.0000  0.7180  0.7659  0.7900  0.6569
]))
; sin-partner detune sign*magnitude (x split), 0 on the cos set and on m=0
(def bat-split (tensor @shape [12 6] @data [
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.0000  0.0000  0.0000  0.0000  0.0000
   0.0000  0.9486  0.8878  0.0000  0.6501 -0.9368
   0.9106 -0.8985  0.0000 -0.6515 -0.6392  0.7225
  -0.7523  0.9978  0.8963  0.9945  0.0000  0.6077
   0.8063 -0.5220 -0.7574 -0.7331  0.8146  0.7571
   0.0000 -0.6238 -0.5059 -0.8460 -0.6003  0.5019
  -0.9150  0.0000  0.6338 -0.9402 -0.9236  0.8199
]))
; m>0 selector: (0,n) slots belong to the scalar proxies
(def bat-asym (tensor @shape [12 6] @data [
   0.0000  1.0000  1.0000  0.0000  1.0000  1.0000
   1.0000  1.0000  0.0000  1.0000  1.0000  1.0000
   1.0000  1.0000  1.0000  1.0000  0.0000  1.0000
   1.0000  1.0000  1.0000  1.0000  1.0000  1.0000
   0.0000  1.0000  1.0000  1.0000  1.0000  1.0000
   1.0000  0.0000  1.0000  1.0000  1.0000  1.0000
   0.0000  1.0000  1.0000  0.0000  1.0000  1.0000
   1.0000  1.0000  0.0000  1.0000  1.0000  1.0000
   1.0000  1.0000  1.0000  1.0000  0.0000  1.0000
   1.0000  1.0000  1.0000  1.0000  1.0000  1.0000
   0.0000  1.0000  1.0000  1.0000  1.0000  1.0000
   1.0000  0.0000  1.0000  1.0000  1.0000  1.0000
]))
; reso head psi at wire 1 contact point, offset -0.55 along the 110.0 deg diameter
(def wpt1 (tensor @shape [12 6] @data [
   0.6082  0.1941 -0.3672 -0.2720 -0.3359 -0.0037
   0.0532 -0.1338 -0.2524  0.2344 -0.2500  0.0913
  -0.1145  0.0602 -0.0893  0.2292  0.2578  0.3581
   0.1747  0.0309  0.1770 -0.0143  0.0584 -0.2110
   0.1191  0.0332  0.1700  0.0814  0.0660 -0.0445
  -0.1944 -0.2428 -0.1330 -0.2565 -0.1656 -0.0479
   0.0000 -0.5333 -0.3081  0.0000  0.1940  0.0101
   0.3017 -0.1123  0.0000  0.0413  0.1443 -0.1582
   0.3145  0.3411 -0.1064  0.1924  0.0000  0.0632
  -0.1008 -0.0848 -0.3067 -0.0810  0.0490 -0.2515
   0.0000  0.0058 -0.0982 -0.2236 -0.1143 -0.2524
  -0.1631  0.0000 -0.1585 -0.0452  0.0956  0.1316
]))
; reso head psi at wire 2 contact point, offset -0.15 along the 110.0 deg diameter
(def wpt2 (tensor @shape [12 6] @data [
   0.9677  0.0943 -0.0541  0.8358 -0.0149  0.1562
   0.0007 -0.1334  0.6211  0.0009 -0.0494  0.0001
   0.1920  0.0030 -0.0000 -0.2239  0.3582  0.0048
  -0.1052  0.1973  0.0007  0.0077 -0.3055 -0.0002
   0.0884  0.0147 -0.1769  0.1725  0.0024  0.0154
  -0.3593 -0.1471 -0.0009  0.0339 -0.2539  0.1232
   0.0000 -0.2590 -0.0454  0.0000  0.0086 -0.4291
   0.0040 -0.1119  0.0000  0.0002  0.0285 -0.0002
  -0.5276  0.0169 -0.0000 -0.1879  0.0000  0.0009
   0.0607 -0.5420 -0.0012  0.0437 -0.2564 -0.0003
   0.0000  0.0026  0.1021 -0.4738 -0.0041  0.0872
  -0.3015  0.0000 -0.0011  0.0060  0.1466 -0.3385
]))
; reso head psi at wire 3 contact point, offset +0.15 along the 110.0 deg diameter
(def wpt3 (tensor @shape [12 6] @data [
   0.9677 -0.0943 -0.0541  0.8358  0.0149 -0.1562
   0.0007 -0.1334  0.6211 -0.0009  0.0494  0.0001
  -0.1920  0.0030  0.0000 -0.2239  0.3582 -0.0048
   0.1052 -0.1973  0.0007  0.0077 -0.3055  0.0002
   0.0884 -0.0147  0.1769 -0.1725  0.0024  0.0154
  -0.3593 -0.1471  0.0009 -0.0339  0.2539 -0.1232
   0.0000  0.2590 -0.0454  0.0000 -0.0086  0.4291
   0.0040 -0.1119  0.0000 -0.0002 -0.0285 -0.0002
   0.5276  0.0169  0.0000 -0.1879  0.0000 -0.0009
  -0.0607  0.5420 -0.0012  0.0437 -0.2564  0.0003
   0.0000 -0.0026 -0.1021  0.4738 -0.0041  0.0872
  -0.3015  0.0000  0.0011 -0.0060 -0.1466  0.3385
]))
; reso head psi at wire 4 contact point, offset +0.55 along the 110.0 deg diameter
(def wpt4 (tensor @shape [12 6] @data [
   0.6082 -0.1941 -0.3672 -0.2720  0.3359  0.0037
   0.0532 -0.1338 -0.2524 -0.2344  0.2500  0.0913
   0.1145  0.0602  0.0893  0.2292  0.2578 -0.3581
  -0.1747 -0.0309  0.1770 -0.0143  0.0584  0.2110
   0.1191 -0.0332 -0.1700 -0.0814  0.0660 -0.0445
  -0.1944 -0.2428  0.1330  0.2565  0.1656  0.0479
   0.0000  0.5333 -0.3081  0.0000 -0.1940 -0.0101
   0.3017 -0.1123  0.0000 -0.0413 -0.1443 -0.1582
  -0.3145  0.3411  0.1064  0.1924  0.0000 -0.0632
   0.1008  0.0848 -0.3067 -0.0810  0.0490  0.2515
   0.0000 -0.0058  0.0982  0.2236 -0.1143 -0.2524
  -0.1631  0.0000  0.1585  0.0452 -0.0956 -0.1316
]))
; proxy (0,1) (0,2) (0,3) displacement at each wire contact point: J0(j_0n |offset|)
(def wpx1a 0.6082) (def wpx1b -0.2720) (def wpx1c -0.2524)
(def wpx2a 0.9677) (def wpx2b 0.8358) (def wpx2c 0.6211)
(def wpx3a 0.9677) (def wpx3b 0.8358) (def wpx3c 0.6211)
(def wpx4a 0.6082) (def wpx4b -0.2720) (def wpx4c -0.2524)
; (0,n) proxy constants: open/edge strike weights, air volume (normalised to (0,1)),
; mic readouts, palm fractions, ratios
(def px-open1 0.2855) (def px-open2 0.0824) (def px-open3 -0.3901)
(def px-edge1 0.0329) (def px-edge2 -0.1034) (def px-edge3 0.1669)
(def px-vol2 -0.2855) (def px-vol3 0.1453)
(def px-bmic1 0.6082) (def px-bmic2 -0.2720) (def px-bmic3 -0.2524)
(def px-rmic1 0.6699) (def px-rmic2 -0.1684) (def px-rmic3 -0.3563)
(def px-palm1 0.9180) (def px-palm2 0.8085) (def px-palm3 0.8033)
(def px-r2 2.2954) (def px-r3 3.5985)
(def mic-abs-sum 10.8251)
;; END GENERATED TABLES

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history bat1 @shape [12 6])   ; batter m>0 modes (t)
(make-tensor-history bat2 @shape [12 6])   ; (t-1)
(make-tensor-history res1 @shape [12 6])   ; reso m>0 modes (t)
(make-tensor-history res2 @shape [12 6])
(make-history stickx)
(make-history stickv)
(make-history bendenv)
(make-history rm1y1) (make-history rm1y2)
(make-history rm2y1) (make-history rm2y2)
(make-history rm3y1) (make-history rm3y2)
(make-history dcx1) (make-history dcy1)
(make-history tonelp) (make-history punchenv)
; (0,n) proxies: batter bp*, reso rp*
(make-history bp1a) (make-history bp1b)
(make-history bp2a) (make-history bp2b)
(make-history bp3a) (make-history bp3b)
(make-history rp1a) (make-history rp1b)
(make-history rp2a) (make-history rp2b)
(make-history rp3a) (make-history rp3b)
(make-history cfh)

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
(def ind-prev (read-history cfh))

; ── Striker: lumped mass in Hertz contact (verbatim family machinery) ───────
(def stick-hard-eff
  (clip (* stick-hard-v (- 1 (* ghost-mix 0.5)) (+ 1 (* edge-mix 2))) 0.0002 0.05))
(def stick-speed-eff (* stick-speed-v (- 1 (* ghost-mix 0.65))))
(def stick-x (gswitch trigger 0.0001 (read-history stickx)))
(def stick-v (gswitch trigger (* -1 (* stick-speed-eff vel-gain))
                      (read-history stickv)))
(def stick-pen (max (* stick-x -1) 0))
(def stick-f (min (* stick-hard-eff stick-pen (sqrt stick-pen)) 0.01))
(def stick-v-next (+ stick-v stick-f))
(def stick-x-next (+ stick-x stick-v-next))
(write-history stickx stick-x-next)
(write-history stickv stick-v-next)
; SPEED loudness compensation (see modal-snare): 1/speed holds RMS flat
(def speed-comp (pow (/ 0.02 (max stick-speed-v 0.002)) 1))
(def strike-force (* stick-f -1 (+ 1 (* scrape-v (noise))) 0.3 speed-comp))

; peak-held stick force: pitch glide + rimshot stick-pivot press (~150 ms)
(def benv (max (* (read-history bendenv) 0.999) (* stick-f 60)))
(write-history bendenv benv)
(def press-total (clip (+ press-v (* edge-mix (min benv 1) 0.9)) 0 1))

; ── Mode frequencies / per-mode decay ───────────────────────────────────────
(def f0 (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 20 2000))
; stretched ratios, then the sin partners detuned by split (percent)
(def bat-r (* (max (+ 1 (* (- bat-ratio 1) stretch-v)) 0.3)
              (+ 1 (* split-v 0.01 bat-split))))
(def r2s (+ 1 (* (- px-r2 1) stretch-v)))
(def r3s (+ 1 (* (- px-r3 1) stretch-v)))
(def bend-mul (* (sqrt (+ 1 (* bend-v benv))) (+ 1 (* press-total 0.04))))
(def f0-res (clip (* f0 pitch2-ratio-v) 20 2000))
(def f-bat (max (* bat-r f0 bend-mul) 20))
(def f-res (max (* bat-r f0-res) 20))
(def w-bat (min (* twopi (/ f-bat samplerate)) 2.83))
(def w-res (min (* twopi (/ f-res samplerate)) 2.83))
; decay law: power tilt x viscous 1/(1+visc(r^2-1))
(defmacro decay-mul (rat)
  (/ (pow rat (* tilt-v -1)) (+ 1 (* visc-v (- (* rat rat) 1)))))
(def dmul-bat (decay-mul bat-r))
(def t60-bat (max (* release-v 0.001 dmul-bat) 0.002))
(def t60-res (max (* release2-v 0.001 dmul-bat) 0.002))
(def r-bat (exp (/ -6.9077553 (* samplerate t60-bat))))
(def r-res (exp (/ -6.9077553 (* samplerate t60-res))))
; palm damping = per-sample state shrink weighted by palm energy fraction
(def dm-bat (max (- 1 (* press-total palm bat-palm)) 0.75))
(def bat1d (* bat1v dm-bat))
(def bat2d (* bat2v dm-bat))

; tip spread relative to the baked 0.06 tip: k_n = 2.405 * ratio_n
(defmacro tipspread (rat)
  (exp (- (* (* 2.405 rat 0.06) (* 2.405 rat 0.06)) (* (* 2.405 rat tip-v) (* 2.405 rat tip-v)))))
(def spread (tipspread bat-r))
(def spread1 (tipspread 1))
(def spread2 (tipspread r2s))
(def spread3 (tipspread r3s))
(def strike-mask-m (* (+ (* bat-open (- 1 edge-mix)) (* bat-edge edge-mix)) spread))
; BRIGHT is a spectral tilt normalised so the mic's summed sensitivity holds.
; Defined here, in the tensor-only section: when this pow loop was scheduled
; right after the contact-mic filters DGenLisp fused those scalar one-poles
; into it (72 updates/sample, contact mic silenced). check_fusion.py guards it.
(def bright-w (pow bat-r bright-v))
(def bright-norm (pow (/ mic-abs-sum (max (sum (* bat-mic-abs bright-w)) 0.001)) 0.15))

; ── Rim hoop: 3 inharmonic modal resonators (verbatim) ──────────────────────
(defmacro rimmode (x f t60s y1h y2h)
  (def w0 (* twopi (/ (clip f 40 18000) samplerate)))
  (def rr (exp (/ -6.9077553 (* samplerate (max t60s 0.001)))))
  (def y1v (read-history y1h))
  (def y2v (read-history y2h))
  (def y (+ (* 2 rr (cos w0) y1v) (* -1 rr rr y2v) x))
  (write-history y2h y1v)
  (write-history y1h y)
  y)
(def rim-t60 (* rim-decay-v 0.001))
(def rim-in (* trigger edge-mix vel-gain 0.2))
(def rim-m1 (rimmode rim-in rim-pitch-v rim-t60 rm1y1 rm1y2))
(def rim-m2 (rimmode rim-in (* rim-pitch-v 2.76) (* rim-t60 0.6) rm2y1 rm2y2))
(def rim-m3 (rimmode rim-in (* rim-pitch-v 5.40) (* rim-t60 0.35) rm3y1 rm3y2))
(def rim-sig (+ rim-m1 (* rim-m2 0.6) (* rim-m3 0.35)))
(def rim-inj (* (clip rim-sig -0.5 0.5) rim-drive-v 0.013))

; ── (0,n) proxies: scalar volume modes of both heads, two-way air coupling ──
(defmacro pm2 (x w0 rr dmv y1v y2v y1h y2h)
  (def y1d (* y1v dmv))
  (def y (+ (* 2 rr (cos w0) y1d) (* -1 rr rr (* y2v dmv)) x))
  (write-history y2h y1d)
  (write-history y1h y)
  y)
(def wb1 (min (* twopi (/ (* f0 bend-mul) samplerate)) 2.83))
(def wb2 (min (* twopi (/ (* f0 bend-mul r2s) samplerate)) 2.83))
(def wb3 (min (* twopi (/ (* f0 bend-mul r3s) samplerate)) 2.83))
(def wr1 (min (* twopi (/ f0-res samplerate)) 2.83))
(def wr2 (min (* twopi (/ (* f0-res r2s) samplerate)) 2.83))
(def wr3 (min (* twopi (/ (* f0-res r3s) samplerate)) 2.83))
(def dmul2 (decay-mul r2s))
(def dmul3 (decay-mul r3s))
(def rb1 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001) 0.002)))))
(def rb2 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 dmul2) 0.002)))))
(def rb3 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 dmul3) 0.002)))))
(def rr1 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001) 0.002)))))
(def rr2 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 dmul2) 0.002)))))
(def rr3 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 dmul3) 0.002)))))
(def dmb1 (- 1 (* press-total palm px-palm1)))
(def dmb2 (- 1 (* press-total palm px-palm2)))
(def dmb3 (- 1 (* press-total palm px-palm3)))
; bounce loss on the reso proxies uses LAST sample's contact indicator
(def dm-p (- 1 (* ind-prev contact-loss-v)))
; air column: relative spring between the heads' volume displacements
(def vol-b (+ bp1y1 (* px-vol2 bp2y1) (* px-vol3 bp3y1)))
(def vol-r (+ rp1y1 (* px-vol2 rp2y1) (* px-vol3 rp3y1)))
(def kc (* head-couple-v 1.27 wr1 wr1))
; separate node for the tensor path: sharing kc between a scalar feedback
; cluster and a tensor elementwise expression trips the DGenLisp fusion bug
; (bd memory dgen-scalar-cluster-fused-into-tensor-loop)
(def kct (* kc 1.0000001))
(def air-b (* kc (- vol-r vol-b)))
(def air-r (* kc (- vol-b vol-r)))
(def sb1 (+ (* strike-force spread1 (+ (* px-open1 (- 1 edge-mix)) (* px-edge1 edge-mix))) (* rim-inj px-edge1) air-b))
(def sb2 (+ (* strike-force spread2 (+ (* px-open2 (- 1 edge-mix)) (* px-edge2 edge-mix))) (* rim-inj px-edge2) (* air-b px-vol2)))
(def sb3 (+ (* strike-force spread3 (+ (* px-open3 (- 1 edge-mix)) (* px-edge3 edge-mix))) (* rim-inj px-edge3) (* air-b px-vol3)))
(def bq1 (pm2 sb1 wb1 rb1 dmb1 bp1y1 bp1y2 bp1a bp1b))
(def bq2 (pm2 sb2 wb2 rb2 dmb2 bp2y1 bp2y2 bp2a bp2b))
(def bq3 (pm2 sb3 wb3 rb3 dmb3 bp3y1 bp3y2 bp3a bp3b))
; reso proxies: air + a little shell bleed of the strike
(def sr1 (+ air-r (* strike-force px-edge1 0.03)))
(def sr2 (+ (* air-r px-vol2) (* strike-force px-edge2 0.03)))
(def sr3 (+ (* air-r px-vol3) (* strike-force px-edge3 0.03)))
(def zp1 (pm2 sr1 wr1 rr1 dm-p rp1y1 rp1y2 rp1a rp1b))
(def zp2 (pm2 sr2 wr2 rr2 dm-p rp2y1 rp2y2 rp2a rp2b))
(def zp3 (pm2 sr3 wr3 rr3 dm-p rp3y1 rp3y2 rp3a rp3b))

; ── Bank updates (m>0 modes; y = 2r cos(w) y1 - r² y2 + x) ─────────────────
; two-way per-mode head coupling from last-sample states (elementwise: the
; compiler only breaks feedback through REDUCES). The reso bank is updated
; BEFORE the wires so they can sense this sample's displacement; nothing
; from the wires flows back into either bank.
(def couple-b (* kct (- res1v bat1v)))
(def couple-r (* kct (- bat1v res1v)))
(def x-bat (* (+ (* strike-force strike-mask-m) (* rim-inj bat-edge) couple-b) bat-asym))
(def bat-next (+ (* 2 r-bat (cos w-bat) bat1d) (* -1 r-bat r-bat bat2d) x-bat))
(def bat-nextc (max (min bat-next 3) -3))
(def x-res (* (+ (* strike-force bat-edge 0.03) couple-r) bat-asym))
(def res-next (+ (* 2 r-res (cos w-res) res1v) (* -1 r-res r-res res2v) x-res))
(def res-nextc (max (min res-next 3) -3))

; ── Snare wires: 12 detuned two-partial strands, each on its own contact ────
; point, with the grid's contact semantics: position PROJECTION. Every
; sample a wire overlaps the head it is pushed back to the surface (and past
; it by the restitution, wire_kick = 1 + e), so while touching it RIDES the
; head's motion — every reso mode reshapes it — and leaves with whatever
; velocity the head had. That is the chaos the impulse-kick model lacked: an
; impulse leaves a 2-pole's period intact, so each wire came back after
; exactly one cycle and the bed rattled as a comb of 12 pitches (measured:
; periodicity 0.39 vs the grid's 0.24). The mic hears the per-sample
; constraint violation, as the grid's contact mic does.
; Wires sense the reso head at 4 zones along the wire diameter (3 strands
; per zone): the full m>0 bank (this sample) + the (0,n) proxies (last
; sample) weighted by their shape at that point.
; strainer: TENSION sets the wire/head gap against a head that moves the
; contact points by ~0.06 at full velocity (modal-snare's 0.001 max gap was
; inaudible: every cycle still hit). Loose (0) = 0.03, only the first big
; swings reach the wires; default 0.85 = 0.0009 (as before); tight (1) =
; -0.003 PRELOAD, the wires ride the head and buzz on every motion. A
; tightened strand also rises in pitch (x0.70 .. x1.05, unity at default).
(def wire-gap (+ (* 0.03 (- 1 snare-tension-v) (- 1 snare-tension-v))
                 0.0002
                 (* -0.02 (max (- snare-tension-v 0.85) 0))))
(def wire-pitch-t (* wire-pitch-v (/ (+ 0.8 (* 0.4 snare-tension-v)) 1.14)))
(def wire-t60 (* wire-decay-v 0.001))
(def rw1 (exp (/ -6.9077553 (* samplerate (max wire-t60 0.002)))))
(def rw2 (exp (/ -6.9077553 (* samplerate (max (* wire-t60 0.4) 0.002)))))
; how a push at the contact point splits over the two partials (sums to one
; unit of displacement at the contact point)
(def push-a1 (/ 1 (+ 1 (* 0.6 wire-tone-v))))
(def push-a2 (* push-a1 0.6))
; contact-point displacements are top-level defs on purpose: a reduce
; written inline next to a scalar feedback cycle gets fused INTO that
; cycle's loop by DGenLisp (72 updates per sample; measured on the DC
; blocker, +24 dB). See bd memory dgen-scalar-cluster-fused-into-tensor-loop.
(defmacro zpt (pt pa pb pc)
  (+ (sum (* res-nextc pt)) (* zp1 pa) (* zp2 pb) (* zp3 pc)))
; 4 contact zones (offsets -0.55 -0.15 +0.15 +0.55), 3 strands each. Each
; reduce is a per-sample 72-wide scalar loop in the generated C (~0.3% of a
; core per voice); 12 individual points measured identical in rattle
; flatness/periodicity to these 4, so they were pure cost.
(def z1 (zpt wpt1 wpx1a wpx1b wpx1c))
(def z2 (zpt wpt2 wpx2a wpx2b wpx2c))
(def z3 (zpt wpt3 wpx3a wpx3b wpx3c))
(def z4 (zpt wpt4 wpx4a wpx4b wpx4c))
(defmacro wire (det zk y1h y2h u1h u2h)
  (def ww1 (min (* twopi (/ (max (* wire-pitch-t det) 20) samplerate)) 2.83))
  (def ww2 (min (* twopi (/ (max (* wire-pitch-t det 2.41) 20) samplerate)) 2.83))
  (def y1v (read-history y1h))
  (def y2v (read-history y2h))
  (def u1v (read-history u1h))
  (def u2v (read-history u2h))
  (def w1 (+ (* 2 rw1 (cos ww1) y1v) (* -1 rw1 rw1 y2v)))
  (def w2 (+ (* 2 rw2 (cos ww2) u1v) (* -1 rw2 rw2 u2v)))
  (def w (+ w1 (* w2 wire-tone-v)))
  (def ov (min (max (- (- zk w) wire-gap) 0) 0.02))
  ; restitution fades out for micro-violations (< 0.001): a preloaded wire
  ; resting on a still head then settles instead of chattering at the
  ; sample rate; physically, restitution drops with impact speed anyway
  (def push (* ov (+ 1 (* (- wire-kick-v 1) (min (* ov 1000) 1)))))
  (write-history y2h y1v)
  (write-history y1h (+ w1 (* push push-a1)))
  (write-history u2h u1v)
  (write-history u1h (+ w2 (* push push-a2)))
  ov)
(make-history w1a) (make-history w1b) (make-history w1c) (make-history w1d)
(make-history w2a) (make-history w2b) (make-history w2c) (make-history w2d)
(make-history w3a) (make-history w3b) (make-history w3c) (make-history w3d)
(make-history w4a) (make-history w4b) (make-history w4c) (make-history w4d)
(make-history w5a) (make-history w5b) (make-history w5c) (make-history w5d)
(make-history w6a) (make-history w6b) (make-history w6c) (make-history w6d)
(make-history w7a) (make-history w7b) (make-history w7c) (make-history w7d)
(make-history w8a) (make-history w8b) (make-history w8c) (make-history w8d)
(make-history w9a) (make-history w9b) (make-history w9c) (make-history w9d)
(make-history w10a) (make-history w10b) (make-history w10c) (make-history w10d)
(make-history w11a) (make-history w11b) (make-history w11c) (make-history w11d)
(make-history w12a) (make-history w12b) (make-history w12c) (make-history w12d)
(def ov6 (wire 1.147 z2 w6a w6b w6c w6d))
(def approach (+ ov6
                 (wire 0.906 z1 w1a w1b w1c w1d)
                 (wire 1.062 z1 w2a w2b w2c w2d)
                 (wire 0.951 z1 w3a w3b w3c w3d)
                 (wire 1.114 z2 w4a w4b w4c w4d)
                 (wire 0.874 z2 w5a w5b w5c w5d)
                 (wire 0.932 z3 w7a w7b w7c w7d)
                 (wire 1.088 z3 w8a w8b w8c w8d)
                 (wire 0.983 z3 w9a w9b w9c w9d)
                 (wire 1.131 z4 w10a w10b w10c w10d)
                 (wire 0.891 z4 w11a w11b w11c w11d)
                 (wire 1.037 z4 w12a w12b w12c w12d)))
(def contact-f (* approach rattle-v))
(def contact-ind (min (* approach 50) 1))
(write-history cfh contact-ind)
(defmacro hp1 (x hist)
  (def lpv (read-history hist))
  (def lpn (+ lpv (* 0.1 (- x lpv))))
  (write-history hist lpn)
  (- x lpn))
(make-history cfl1) (make-history cfl2)
(def contact-hp (hp1 (hp1 contact-f cfl1) cfl2))

; ── Output taps ─────────────────────────────────────────────────────────────
; BRIGHT weights are defined up with the other tensor tables (fusion, see wires)
(def mic-top (* 14 bright-norm (+ (sum (* bat-nextc bat-mic bright-w))
                      (* bq1 px-bmic1)
                      (* bq2 px-bmic2 (pow r2s bright-v))
                      (* bq3 px-bmic3 (pow r3s bright-v)))))
(def mic-bot (* 14 bright-norm (+ (sum (* res-nextc res-mic bright-w))
                      (* zp1 px-rmic1)
                      (* zp2 px-rmic2 (pow r2s bright-v))
                      (* zp3 px-rmic3 (pow r3s bright-v)))))
; contact mic: 60 (projection contact reports the per-sample constraint violation,
; ~8 dB smaller than the impulse model's overlap for the same rattle)
(def mic-snap (* contact-hp 60))
(def mixdown (+ mic-top (* mic-bot bottom-mix-v) (* mic-snap snares-v)
                (* rim-sig rim-level-v 0.1)))

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
(def dcin bodied)
(def dcy (+ (- dcin (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 dcin)
(write-history dcy1 dcy)
; debug tap (see `dbg`)
(def sel1 (clip (- 1 (abs (- dbg 1))) 0 1))
(def sel2 (clip (- 1 (abs (- dbg 2))) 0 1))
(def sel3 (clip (- 1 (abs (- dbg 3))) 0 1))
(def sel0 (clip (- 1 (+ sel1 sel2 sel3)) 0 1))
(out (+ (* dcy level-v vel-gain sel0) (* z2 sel1) (* (read-history w6a) sel2) (* contact-f sel3)) 1 @name audio)
