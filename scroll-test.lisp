;; Scroll container test — border-styled scroll box

;; Border background: stroked rounded rect (like CSS border: 1px solid)
(defwidget scroll-border
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill (sdf/rounded-rect (* width 0.98) (* height 0.98) 0.03)
      (material
        :color (mix :black :gray (* (cos  (* (cos itime) (+  (sin (* itime 0.3)) x) y))))))
    (sdf/stroke (sdf/rounded-rect (* width 0.98) (* height 0.98) 0.03) 0.005 :white)))

(effect (v-stack :padding 1 :gap 1
  (label "Fixed Header - Scroll the list below" :color :white :bg :transparent :font-size 16)

  ;; The box wraps the scroll with a visible border
  (box :background "scroll-border" :padding 2 :flex 1
    (scroll :flex 1
      (v-stack :gap 0.25 :padding 0.5
        (label "Item 1"  :bg :transparent)
        (label "Item 2"  :bg :transparent)
        (label "Item 3"  :bg :transparent)
        (label "Item 4"  :bg :transparent)
        (label "Item 5"  :bg :transparent)
        (label "Item 6"  :bg :transparent)
        (label "Item 7"  :bg :transparent)
        (label "Item 8"  :bg :transparent)
        (label "Item 9"  :bg :transparent)
        (label "Item 10" :bg :transparent)
        (label "Item 11" :bg :transparent)
        (label "Item 12" :bg :transparent)
        (label "Item 13" :bg :transparent)
        (label "Item 14" :bg :transparent)
        (label "Item 15" :bg :transparent)
        (label "Item 16" :bg :transparent)
        (label "Item 17" :bg :transparent)
        (label "Item 18" :bg :transparent)
        (label "Item 19" :bg :transparent)
        (label "Item 20" :bg :transparent)
        (label "Item 21" :bg :transparent)
        (label "Item 22" :bg :transparent)
        (label "Item 23" :bg :transparent)
        (label "Item 24" :bg :transparent)
        (label "Item 25" :bg :transparent)
        (label "Item 26" :bg :transparent)
        (label "Item 27" :bg :transparent)
        (label "Item 28" :bg :transparent)
        (label "Item 29" :bg :transparent)
        (label "Item 30" :bg :transparent)
        (label "Item 31" :bg :transparent)
        (label "Item 32" :bg :transparent)
        (label "Item 33" :bg :transparent)
        (label "Item 34" :bg :transparent)
        (label "Item 35" :bg :transparent)
        (label "Item 36" :bg :transparent)
        (label "Item 37" :bg :transparent)
        (label "Item 38" :bg :transparent)
        (label "Item 39" :bg :transparent)
        (label "Item 40" :bg :transparent)
        (label "Item 41" :bg :transparent)
        (label "Item 42" :bg :transparent)
        (label "Item 43" :bg :transparent)
        (label "Item 44" :bg :transparent)
        (label "Item 45" :bg :transparent)
        (label "Item 46" :bg :transparent)
        (label "Item 47" :bg :transparent)
        (label "Item 48" :bg :transparent)
        (label "Item 49" :bg :transparent)
        (label "Item 50 - End of list" :bg :transparent :color :green))))))
