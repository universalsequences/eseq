; ======================================================================
; Virtual analog VCF engine (plaits/dsp/engine2/virtual_analog_vcf_engine.cc):
; VariableShapeOscillator (saw..PW square) + square sub at f0*0.501 into
; two SVFs (resonant LP/HP, then a second gentle LP stage).
; ======================================================================

; stmlib SoftClip (SoftLimit within +-3).
(defmacro vavcf-softclip (x)
  (def c (clip x -3 3))
  (/ (* c (+ 27 (* c c))) (+ 27 (* 9 c c))))

; VariableShapeOscillator::ComputeNaiveSample.
(defmacro vavcf-vso-naive (phase pw su sd tri sq)
  (def lo (lt phase pw))
  (def square (- 1 lo))
  (def triangle (gswitch lo (* phase su) (- 1 (* (- phase pw) sd))))
  (def s1 (+ phase (* (- square phase) sq)))
  (+ s1 (* (- triangle s1) tri)))

; One step of VariableShapeOscillator without sync. State in: slave phase
; sp, carried next_sample ns, high flag, previous pw ppw. Returns
; (sp' ns' high' this_sample).
(defmacro vavcf-vso-step (sp ns high ppw sf pw tri sq)
  (def su (/ 1 pw))
  (def sd (/ 1 (- 1 pw)))
  (def sfs (max sf 0.000000001))
  (def spa (+ sp sf))
  (def rise (* (- 1 high) (gte spa pw)))
  (def den (+ (- ppw pw) sf))
  (def tr (/ (- spa pw) (gswitch (lt (abs den) 0.000000001) 0.000000001 den)))
  (def tstep (* (+ su sd) sf tri))
  (def high1 (max high rise))
  (def fall (* high1 (gte spa 1)))
  (def sp1 (- spa fall))
  (def tf (/ sp1 sfs))
  (def high2 (* high1 (- 1 fall)))
  (def this1 (+ ns
                (* rise (- (* sq (pl-this-blep tr)) (* tstep (pl-this-integrated-blep tr))))
                (- (* fall (- (* (- 1 tri) (pl-this-blep tf)) (* tstep (pl-this-integrated-blep tf)))))))
  (def nxt1 (+ (* rise (- (* sq (pl-next-blep tr)) (* tstep (pl-next-integrated-blep tr))))
               (- (* fall (- (* (- 1 tri) (pl-next-blep tf)) (* tstep (pl-next-integrated-blep tf)))))))
  (tuple sp1 (+ nxt1 (vavcf-vso-naive sp1 pw su sd tri sq)) high2 this1))

; VariableShapeOscillator::Render(frequency, pw, waveshape) -> 2*x - 1.
(defmacro vavcf-vso (freq pw shape bstart bphase)
  (make-history h_sp)
  (make-history h_ns)
  (make-history h_hi)
  (make-history h_ppw)
  (def f (min freq 0.25))
  (def pwc (gswitch (gte f 0.25) 0.5 (clip pw (* f 2) (- 1 (* 2 f)))))
  (def sf (pl-interp f bstart bphase 0.01))
  (def pwi (pl-interp pwc bstart bphase 0.5))
  (def ws (pl-interp shape bstart bphase 0))
  (def sq (* (max (- ws 0.5) 0) 2))
  (def tri (max (- 1 (* ws 2)) 0))
  (def ppw (+ 0.5 (read-history h_ppw)))
  (def (sp1 ns1 hi1 this1) (vavcf-vso-step (read-history h_sp) (read-history h_ns) (read-history h_hi) ppw sf pwi tri sq))
  (write-history h_sp sp1)
  (write-history h_ns ns1)
  (write-history h_hi hi1)
  (write-history h_ppw (- pwi 0.5))
  (- (* 2 this1) 1))

(defmacro plaits-vavcf (bstart bphase rise high patched note timbre morph harmonics accent)
  (def f0 (pl-note-to-freq note))
  (def shape (clip (+ (* (- morph 0.25) 2) 0.5) 0.5 1))
  (def pw (clip (gswitch (gt morph 0.75) (- 2.5 (* morph 2)) (+ (* (- morph 0.5) 2) 0.5)) 0.5 0.98))
  (def sub_gain (* (max (- (abs (- morph 0.5)) 0.3) 0) 5))
  (def osc (vavcf-vso f0 pw shape bstart bphase))
  (def sub (vavcf-vso (* f0 0.501) 0.5 1 bstart bphase))
  (def cutoff_t (* f0 (pl-hold (pl-semi-ratio (* (- timbre 0.2) 120)) bstart)))
  (def stage2_t (clip (- 1 (* (- harmonics 0.4) 4)) 0 1))
  (def res (* 2.667 (max (- (abs (- harmonics 0.5)) 0.125) 0)))
  (def res2 (* res res))
  (def q_t (* res2 res2 48))
  (def gain_t (clip (+ (- harmonics 0.7) 0.85) (- 0.7 (* res2 0.3)) 1))
  (def sgain (pl-interp sub_gain bstart bphase 0))
  (def cutoff (min (pl-interp cutoff_t bstart bphase 0) 0.25))
  (def q (pl-interp q_t bstart bphase 0))
  (def stage2 (pl-interp stage2_t bstart bphase 0))
  (def gain (pl-interp gain_t bstart bphase 0))
  (def g (pl-tan-fast cutoff))
  (def input (vavcf-softclip (* (+ osc (* sub sgain)) gain)))
  (def (lp1 bp1 hp1) (pl-svf input g (/ 1 (+ 0.5 q))))
  (def lpa (vavcf-softclip (* lp1 gain)))
  (def (lp2 bp2 hp2) (pl-svf lpa g (/ 1 (+ 0.5 (* 0.025 q)))))
  (def lpb (+ lpa (* stage2 (- (vavcf-softclip lp2) lpa))))
  (tuple lpb (vavcf-softclip (* hp1 gain))))
