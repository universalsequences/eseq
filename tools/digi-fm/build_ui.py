#!/usr/bin/env python3
"""Generate routing/spectrum graphics from the same data as the FM core."""
import sys
sys.dont_write_bytecode = True
from pathlib import Path
from build import ALGORITHMS, PARTIALS, anchors, DEST

HERE=Path(__file__).resolve().parent

def build_ui():
    lines=[(HERE/'ui-controls.lisp').read_text()]
    emit=lines.append
    emit('''(defwidget df-routing
  :width 7.8 :height 1.65
  :state (mode selected) :bindable (selected)
  :shader
  (let ((ink (if (> selected .5) :control-on-bg :control-on-fg)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (if (> selected .5) :control-on-fg :control-on-bg))''')
    pos=dict(c=(-.55,.55),a=(-.55,-.2),b1=(.4,.25),b2=(.4,-.6))
    for i,algo in enumerate(ALGORITHMS,1):
        emit(f'      (if (= mode {i}) (sdf/layer')
        for src,dst in algo['edges']:
            x,y=pos[src];a,b=pos[dst]
            emit(f'        (sdf/stroke (sdf/line (* width {x}) (* height {y}) (* width {a}) (* height {b})) .04 ink)')
        x,y=pos[algo['feedback']]
        emit(f'        (sdf/stroke (sdf/translate (* width {x-.07}) (* height {y-.1}) (sdf/rect (* width .1) (* height .16))) .04 ink)')
        for bus,xx in [('x',-.35),('y',.55)]:
            for op,env in algo[bus]:
                x,y=pos[op]
                emit(f'        (sdf/stroke (sdf/line (* width {x}) (* height {y}) (* width {xx}) (* height .88)) {".04" if env else ".018"} ink)')
        emit('      ) (rgba 0 0 0 0))')
    for x,y in pos.values():
        emit(f'      (sdf/fill (sdf/translate (* width {x}) (* height {y}) (sdf/rect (* width .08) (* height .12))) ink)')
    emit(')))')
    emit('''(defwidget df-spectrum
  :width 32 :height 4.7
  :state (harm) :bindable (harm)
  :shader
  (let ((position (abs harm)))
    (sdf/layer''')
    for h in range(1,PARTIALS+1):
        coeff=anchors(h)
        terms=[f'(* {a:.10f} (max 0 (- 1 (abs (- position {i})))))' for i,a in enumerate(coeff) if a]
        amp='(+ '+' '.join(terms)+')'
        x=-.94+(h-.5)*1.88/PARTIALS
        emit(f'      (let ((amp {amp})) (sdf/fill (sdf/translate (* width {x:.8f}) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))')
    emit(')))')
    emit((HERE/'ui-body.lisp').read_text())
    (DEST/'ui.lisp').write_text('\n'.join(lines))

if __name__=='__main__':build_ui()
