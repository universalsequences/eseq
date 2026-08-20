;; Instrument source tabs and source parameter grids.
(module eseq.effects.instrument-sources)
(import eseq.effects.state :refer (instrument-source-tab))
(import eseq.effects.param-grid :as pg)

(export)

;; No compat aliases: neither def has a caller outside this file (lisp or
;; Rust), so both are private.
;; The "instrument-source-…" string below is a subtree key built for
;; pg/fx-param-row, which attaches it — byte-identical, hazard (e).
(def sources-grid (sections)
  (h-stack :gap 2
    (each sections |section si|
      (v-stack :gap 0.25
        (label (get section :name) :font-size 14 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (pg/fx-param-row p false
            (str "instrument-source-" si "-param-" (get p :idx))))))))

(def source-tabs (inst)
  (if (> (len (get inst :sources)) 0)
    (tabs :items (get inst :source-names)
      :bind eseq.effects.state/instrument-source-tab
      :compact true
      :gap 0.75
      :tab-padding 0.5
      :header-height 1
      (each (get inst :sources) |section si|
        (pg/fx-param-grid (get section :params) false)))
    (sources-grid (get inst :sources))))
