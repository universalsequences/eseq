; ======================================================================
; Voice and Part (elements/dsp/voice.cc, part.cc): block clock and gate
; flags, the exciter envelope, the three exciters and their mix, the
; resonator models (modal, string, five chord strings) or the Ominous voice,
; then the Part's stereo mixdown, pre-clipping and reverb, and the codec's
; SoftConvert output stage.
; Expects from the header: gate_now trig_now midi_now strength_now model_now
; (and gate_o trig_o midi_o p_geometry_o p_brightness_o p_damping_o
; p_position_o: copies for the Ominous voice that share no history with the
; other paths)
; space_now p_contour p_bow p_bow_timbre p_blow p_blow_meta p_blow_timbre
; p_strike p_strike_meta p_strike_timbre p_signature p_geometry p_brightness
; p_damping p_position p_mod_freq p_mod_offset p_rev_diffusion p_rev_lp, and
; the noise draws u_bow u_blow u_s1 u_s2 u_s3 u_st0 .. u_st4.
; ======================================================================

(def v_lut_coarse (tensor @shape [257 1] @file "lut_accent_gain_coarse.json"))
(def v_lut_fine (tensor @shape [257 1] @file "lut_accent_gain_fine.json"))

; ---- block clock (blk samples: the module's 0.5 ms audio block) ----
(make-history h_bp)
(def bphase (read-history h_bp))
(write-history h_bp (el-block-step bphase))
(def bstart (eq bphase 0))

; ---- gate flags, per block; a trigger pulse re-strikes a held note ----
(def g_blk (el-hold (gt gate_now 0.5)))
(make-history h_gprev)
(def g_prev (el-hold (read-history h_gprev)))
(write-history h_gprev g_blk)
(make-history h_pending)
(def pending (max (read-history h_pending) (gt trig_now 0.5)))
(write-history h_pending (gswitch bstart 0 pending))
(def retrig (el-hold pending))
(def rising (* g_blk (max (- 1 g_prev) retrig)))
(def falling (* (- 1 g_blk) g_prev))

; ---- performance state and patch, per block ----
; (model_sel, the model parameter itself, gates the resonator models below)
(def model_sel (clip (floor (+ model_now 0.5)) 0 3))
(def model_b (el-hold model_sel))
(def midi_b (el-hold midi_now))
(def strength_b (el-hold (clip strength_now 0 1)))
(def contour (el-hold (clip p_contour 0 1)))
(def bow (el-hold (clip p_bow 0 1)))
(def bow_timbre (el-hold (clip p_bow_timbre 0 0.9995)))
(def blow (el-hold (clip p_blow 0 1)))
(def blow_meta (el-hold (clip p_blow_meta 0 0.9995)))
(def blow_timbre (el-hold (clip p_blow_timbre 0 0.9995)))
(def strike (el-hold (clip p_strike 0 1)))
(def strike_meta (el-hold (clip p_strike_meta 0 0.9995)))
(def strike_timbre (el-hold (clip p_strike_timbre 0 0.995)))
(def signature (el-hold (clip p_signature 0 0.999)))
(def geometry (el-hold (clip p_geometry 0 0.9995)))
(def brightness (el-hold (clip p_brightness 0 0.9995)))
(def damping (el-hold (clip p_damping 0 0.9995)))
(def position (el-hold (clip p_position 0 0.9995)))
(def space_b (el-hold (clip space_now 0 2)))
(def is_strings (eq model_b 2))
(def is_omin (eq model_b 3))

; Part: note -> lut_midi_to_f_high[] * lut_midi_to_f_low[] (cycles per
; 32 kHz sample, capped at 12 kHz).
(def pitch_i (el-hold (clip (el-trunc (* (+ midi_b 48) 256)) 0 65535)))
(def pitch_hi (floor (/ pitch_i 256)))
(def f32 (el-hold (* (peek om_lut_f_high pitch_hi 0) (peek om_lut_f_low (- pitch_i (* pitch_hi 256)) 0))))

; ---- exciter envelope (Voice::Process) ----
(def env_lo (lt contour 0.4))
(def env_mid (lt contour 0.6))
(def env_a (gswitch env_lo (+ (* contour 0.75) 0.15) (gswitch env_mid 0.45 (+ (* (- 1 contour) 0.75) 0.15))))
(def env_dr (gswitch env_mid (gswitch env_lo (* env_a 1.8) 0.81) (* env_a 1.8)))
(def env_s (gswitch env_lo 0 (gswitch env_mid (* (- contour 0.4) 5) 1)))
(def env_gain (el-hold (gswitch env_lo (- 5 (* contour 10)) 1)))
(def env_val (el-hold (* (el-envelope rising falling g_blk env_a env_dr env_s) env_gain)))
(def e_ramp (el-interp env_val 0))

; ---- exciters ----
(def bow_sig (exc-bow rising (* bow_timbre (+ 0.4 (* 0.6 brightness))) u_bow))
(def bl (* blow 1.5))
(def tube_level (el-hold (gswitch (gt bl 1) (* (- bl 1) 2) 0)))
(def blow_lvl (el-hold (gswitch (lt bl 1) (* bl 0.4) 0.4)))
(def blow_raw (exc-granular blow_meta blow_timbre signature u_blow))
(def blow_tube (exc-tube f32 env_val damping tube_level blow_raw (* tube_level 0.5)))
(def blow_sig (exc-diffuser (* blow_tube blow_lvl)))
(def (strike_raw strike_damp) (exc-strike rising g_blk strike_meta strike_timbre signature u_s1 u_s2 u_s3))
(def sl0 (* strike 1.25))
(def bleed (el-hold (gswitch (gt sl0 1) (* (- sl0 1) 2) 0)))
(def sl (el-hold (* (min sl0 1) 1.5)))
(def st_ramp (el-interp (* strength_b 256) 0))
(def st_i (floor st_ramp))
(def accent (* (peek v_lut_coarse st_i 0) (peek v_lut_fine (floor (* 256 (- st_ramp st_i))) 0)))
(def bow_strength (* e_ramp bow))
(def strike_acc (* strike_raw accent))
(def raw (* 0.5 (+ (* bow_sig bow_strength 0.125 accent) (* blow_sig e_ramp accent) (* strike_acc sl))))
(def bs0 (el-hold bow_strength))
(def damping_eff (el-hold (max 0 (- damping (* strike_damp sl 0.125) (* (- 1 bs0) bow 0.0625)))))

; ---- resonator models: only the selected one runs; the others hold their
; state, as the module's Voice runs one model at a time. The gates test the
; parameter itself, so they skip whole process calls.

; ---- modal resonator ----
(def (m_center m_sides)
  (block-gate (eq model_sel 0)
    (el-resonator f32 geometry brightness damping_eff position p_mod_freq p_mod_offset raw bow_strength)))

; ---- string models: raw is DC-blocked and normalized, then 1 or 5 strings
(make-history h_dcx)
(make-history h_dcy)
(def dcy (+ (* (read-history h_dcy) (- 1 (/ 10 samplerate))) raw (- (read-history h_dcx))))
(write-history h_dcx raw)
(write-history h_dcy dcy)
(def sraw (* dcy (gswitch is_strings 0.2 1)))
(make-history h_chord)
(def chord (* geometry 10))
(def chord_prev (read-history h_chord))
(def chord_i (el-hold (clip (el-trunc (+ chord (gswitch (gt chord chord_prev) -0.1 0.1) 0.5)) 0 10)))
(write-history h_chord chord_i)
(def tr2 (el-hold (selector (+ chord_i 1) 0 3 3 3 3 7 4 4 4 4 5)))
(def tr3 (el-hold (selector (+ chord_i 1) 0.01 7 7 7 7 12 7 7 7 7 7)))
(def tr4 (el-hold (selector (+ chord_i 1) 12 10 12 14 17 19 17 14 12 11 12)))
(def disp0 (el-hold (str-dispersion (gswitch is_strings
  (gswitch (lt brightness 0.5) 0 (* (- brightness 0.5) -0.4)) geometry))))
(def disp_n (el-hold (str-dispersion (gswitch (lt brightness 0.5) 0 (* (- brightness 0.5) -0.4)))))
(def sraw_n (* is_strings sraw))
(def str_line0 (tensor @shape [8192]))
(def str_line1 (tensor @shape [8192]))
(def str_line2 (tensor @shape [8192]))
(def str_line3 (tensor @shape [8192]))
(def str_line4 (tensor @shape [8192]))
(def str_ap0 (tensor @shape [4096]))
(def str_ap1 (tensor @shape [4096]))
(def str_ap2 (tensor @shape [4096]))
(def str_ap3 (tensor @shape [4096]))
(def str_ap4 (tensor @shape [4096]))
; The chord strings' pitches (held by the voice, outside the gate).
(def f_chord1 (el-hold (* f32 (el-semi-ratio -12))))
(def f_chord2 (el-hold (* f32 (el-semi-ratio tr2))))
(def f_chord3 (el-hold (* f32 (el-semi-ratio tr3))))
(def f_chord4 (el-hold (* f32 (el-semi-ratio tr4))))
(def (so0 sa0) (block-gate (* (gte model_sel 1) (lte model_sel 2))
  (el-string str_line0 str_ap0 f32 disp0 brightness damping_eff position sraw u_st0)))
(def (so1 sa1 so2 sa2 so3 sa3 so4 sa4) (block-gate (eq model_sel 2) (do
  (def (o1 a1) (el-string str_line1 str_ap1 f_chord1 disp_n brightness damping_eff position sraw_n u_st1))
  (def (o2 a2) (el-string str_line2 str_ap2 f_chord2 disp_n brightness damping_eff position sraw_n u_st2))
  (def (o3 a3) (el-string str_line3 str_ap3 f_chord3 disp_n brightness damping_eff position sraw_n u_st3))
  (def (o4 a4) (el-string str_line4 str_ap4 f_chord4 disp_n brightness damping_eff position sraw_n u_st4))
  (tuple o1 a1 o2 a2 o3 a3 o4 a4))))
(def str_l (+ so0 (* is_strings (+ so1 so2 so3 so4))))
(def str_r (+ sa0 (* is_strings (+ sa1 sa2 sa3 sa4))))

; ---- Ominous voice (only runs when selected) ----
; The voice keeps its own block clock, flags and holds, from the header's own
; copies of the note inputs (gate_o trig_o midi_o) and of the swept patch
; values (p_*_o), so its gate owns everything the voice runs.
(make-history h_bpo)
(def bphase_o (read-history h_bpo))
(write-history h_bpo (el-block-step bphase_o))
(def bstart_o (eq bphase_o 0))
(defmacro om-in (x) (om-hold (clip x 0 0.9995)))
(def go_blk (om-hold (gt gate_o 0.5)))
(make-history h_goprev)
(def go_prev (om-hold (read-history h_goprev)))
(write-history h_goprev go_blk)
(make-history h_opending)
(def o_pending (max (read-history h_opending) (gt trig_o 0.5)))
(write-history h_opending (gswitch bstart_o 0 o_pending))
(def o_rising (* go_blk (max (- 1 go_prev) (om-hold o_pending))))
(def o_falling (* (- 1 go_blk) go_prev))
(def (o_raw o_center o_sides)
  (block-gate (eq model_sel 3)
    (el-ominous o_rising o_falling go_blk (om-hold midi_o) (om-hold (clip strength_now 0 1))
                (om-in p_contour) (om-in p_bow) (om-in p_bow_timbre)
                (om-in p_blow) (om-in p_blow_meta) (om-in p_blow_timbre)
                (om-in p_strike) (om-in p_strike_meta) (om-in p_strike_timbre)
                (om-in p_signature) (om-in p_geometry_o) (om-in p_brightness_o)
                (om-in p_damping_o) (om-in p_position_o) (om-hold p_mod_offset))))

; ---- the active model's raw / center / sides ----
(def bleed_sig (* bleed strike_acc))
(def v_raw (selector (+ model_b 1) raw sraw sraw o_raw))
(def v_center (selector (+ model_b 1) (+ m_center bleed_sig) (+ (- str_l str_r) bleed_sig)
                        (+ (- str_l str_r) bleed_sig) o_center))
(def v_sides (selector (+ model_b 1) m_sides (+ str_l str_r) (+ str_l str_r) o_sides))

; ---- Part: space -> raw gain, stereo spread, reverb; pre-clipping ----
(def space_c (min space_b 1))
(def raw_gain (el-hold (gswitch (lte space_c 0.05) 1 (gswitch (lte space_c 0.1) (- 2 (* space_c 20)) 0))))
(def space_s (gswitch (gte space_c 0.1) (- space_c 0.1) 0))
(def spread (el-hold (min space_s 0.7)))
(def rv_amount (el-hold (gswitch (gte space_s 0.5) (- space_s 0.5) 0)))
(def frozen (el-hold (gte space_b 1.75)))
(def side (* v_sides spread))
(def main0 (- v_center side))
(def left0 (+ v_center side))
(def aux0 (+ left0 (* (- v_raw left0) raw_gain)))
(def main1 (gswitch is_omin main0 (el-soft-limit main0)))
(def aux1 (gswitch is_omin aux0 (el-soft-limit aux0)))
(def (part_main part_aux)
  (el-reverb main1 aux1 rv_amount (gswitch frozen 0 0.2) (gswitch frozen 1 (+ 0.35 (* 1.2 rv_amount)))
             p_rev_diffusion (gswitch frozen 1 p_rev_lp)))
; codec: SoftConvert (SoftLimit(x / 2), clipped to the 16-bit range)
(def out_main (clip (el-soft-limit (* part_main 0.5)) -1 1))
(def out_aux (clip (el-soft-limit (* part_aux 0.5)) -1 1))
