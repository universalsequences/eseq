; Modalys material table (Object Properties > Material) by index:
;   0 uranium 19100 / 2.08e11   1 steel 7850 / 2.0e11   2 brass 8500 / 1.0e11
;   3 aluminium 2700 / 7.0e10   4 granite 2700 / 4.0e10 5 nylon 1140 / 3.0e9
;   6 carbon fiber 1400 / 1.1e12 7 diamond 3500 / 1.22e12
;   8 spruce 450 / 1.1e10 (along the grain; not in the Modalys table)
; Returns (density kg/m^3, young N/m^2). Only E/rho shapes a modal object's
; partials at a fixed pitch; density alone matters for mass-scaled coupling.
(defmacro modal-material (material)
  (def mat (floor (+ material 0.5)))
  (def density
    (gswitch (lt mat 0.5) 19100 (gswitch (lt mat 1.5) 7850 (gswitch (lt mat 2.5) 8500
    (gswitch (lt mat 3.5) 2700 (gswitch (lt mat 4.5) 2700 (gswitch (lt mat 5.5) 1140
    (gswitch (lt mat 6.5) 1400 (gswitch (lt mat 7.5) 3500 450)))))))))
  (def young
    (gswitch (lt mat 0.5) 2.08e11 (gswitch (lt mat 1.5) 2.0e11 (gswitch (lt mat 2.5) 1.0e11
    (gswitch (lt mat 3.5) 7.0e10 (gswitch (lt mat 4.5) 4.0e10 (gswitch (lt mat 5.5) 3.0e9
    (gswitch (lt mat 6.5) 1.1e12 (gswitch (lt mat 7.5) 1.22e12 1.1e10)))))))))
  (tuple density young))
