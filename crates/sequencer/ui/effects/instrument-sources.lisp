;; Instrument source tabs and source parameter grids.
(def instrument-sources-grid (sections)
  (h-stack :gap 2 
    (each sections |section si|
      (v-stack :gap 0.25
        (label (get section :name) :font-size 14 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (fx-param-row p false
            (str "instrument-source-" si "-param-" (get p :idx))))))))

(def instrument-source-tabs (inst)
  (if (> (len (get inst :sources)) 0)
    (tabs :items (get inst :source-names)
      :bind instrument-source-tab
      :compact true
      :gap 0.75
      :tab-padding 0.5
      :header-height 1
      (each (get inst :sources) |section si|
        (fx-param-grid (get section :params) false)))
    (instrument-sources-grid (get inst :sources))))
