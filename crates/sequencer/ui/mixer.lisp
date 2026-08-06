;; ui/mixer.lisp — Horizontal DAW-style mixer.
;; Renders to *mixer* buffer. Loaded by ui/main.lisp.

(load "@/ui/track-collapse.lisp")

(def track-peak (i)
  (bind-seq (str "track-peak-" i)))

(def bus-peak-l (i)
  (if (= (nth SEQ.bus-names i) "Mix")
    (bind-seq "master-peak-l")
    (bind-seq (str "bus-peak-" i))))

(def bus-peak-r (i)
  (if (= (nth SEQ.bus-names i) "Mix")
    (bind-seq "master-peak-r")
    (bind-seq (str "bus-peak-" i))))

(def mixer-v2-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

(def mixer-v2-track-selected-binding (i)
  (bind-seq (str "track-selected-" i)))

(def mixer-v2-track-delete-target-binding (i)
  (bind-seq (str "mixer-track-delete-target-" i)))

(def mixer-v2-track-pattern-cell-active-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-active-" track "-" pattern-id)))

(def mixer-v2-track-pattern-cell-assigned-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-assigned-" track "-" pattern-id)))

(def mixer-v2-track-pattern-cell-override-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-override-" track "-" pattern-id)))

(def mixer-v2-track-pattern-cell-selected-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-selected-" track "-" pattern-id)))

(def mixer-v2-track-color (i)
  (if (< i (len SEQ.track-colors))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

(def mixer-v2-track-color-r (i muted)
  (let ((r (nth (mixer-v2-track-color i) 0)))
    (if muted (+ (* r 0.34) (* 0.10 0.66)) r)))

(def mixer-v2-track-color-g (i muted)
  (let ((g (nth (mixer-v2-track-color i) 1)))
    (if muted (+ (* g 0.34) (* 0.10 0.66)) g)))

(def mixer-v2-track-color-b (i muted)
  (let ((b (nth (mixer-v2-track-color i) 2)))
    (if muted (+ (* b 0.34) (* 0.11 0.66)) b)))

(def mixer-v2-strip-bg (selected muted)
  (if selected
    :mixer-strip-selected-bg
    (if muted
      :mixer-strip-muted-bg
      :mixer-strip-bg)))

(def mixer-v2-strip-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(def mixer-v2-button-bg (active)
  (if active
    (rgba 0.95 0.48 0.18 1.0)
    :mixer-control-bg))

(def mixer-v2-arm-bg (active)
  (if active
    (rgba 0.95 0.20 0.18 1.0)
    :mixer-control-bg))

(def mixer-v2-pointer-volume (sy)
  (max 0.0 (min 1.0 (* 0.5 (- 1.0 sy)))))

(def mixer-v2-event-volume (event)
  (mixer-v2-pointer-volume (get event :sy)))

(def mixer-v2-select-track (i)
  (do
    (set! selected-bus -1)
    (mixer-v2-clear-delete-target)
    (seq-set-track i)
    (host-command "reveal-sequencer-track" (dict :track i))))

(def mixer-v2-select-track-delete-target (i)
  (do
    (set! selected-bus -1)
    (seq-set-track i)
    (host-command "reveal-sequencer-track" (dict :track i))
    (seq-set-delete-target :mixer-track (dict :track i))))

(def mixer-v2-activate-track-control (i)
  (do
    (set! selected-bus -1)
    (mixer-v2-clear-delete-target)))

;; cmd/super/meta-click on a mixer strip toggles multi-select membership.
(def mixer-v2-multi-select-click? (event)
  (or (get event :cmd) (get event :super) (get event :meta)))

(def mixer-v2-toggle-track-select (i)
  (do
    (set! selected-bus -1)
    (mixer-v2-clear-delete-target)
    (seq-toggle-track-selected i)
    (host-command "reveal-sequencer-track" (dict :track i))))

;; Plain click = single-select; cmd-click = toggle membership in the set.
(def mixer-v2-track-body-click (event i)
  (if (mixer-v2-multi-select-click? event)
    (mixer-v2-toggle-track-select i)
    (mixer-v2-select-track i)))

;; The label preserves its delete-target gesture on a plain click.
(def mixer-v2-track-label-click (event i)
  (if (mixer-v2-multi-select-click? event)
    (mixer-v2-toggle-track-select i)
    (mixer-v2-select-track-delete-target i)))

(def mixer-v2-select-bus (i)
  (do
    (seq-clear-selection)
    (seq-clear-delete-target)
    (set! selected-bus i)))

(def mixer-v2-drop-sample-on-track (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((path (get payload :path))
        (track (get target :track)))
      (do
        (mixer-v2-clear-delete-target)
        (if path
          (sbrowser-drop-sample-on-track event)
          (status "Drop a sample file, not a folder"))))))

(def mixer-v2-drop-sample-new-track (event)
  (if (= (get event :drag-type) "track-badge")
    (mixer-v2-drop-track-out-of-group event)
    (let ((payload (get event :payload)))
      (let ((path (get payload :path))
            (name (get payload :name)))
        (do
          (mixer-v2-clear-delete-target)
          (if (= (get event :drag-type) "sound")
            (if path
              (host-command "add-track-from-sound" (dict :path path))
              (status "Drop a Sound item, not a folder"))
            (if (= (get event :drag-type) "instrument")
              (if name
                (do
                  (set! sbrowser-loading-instrument-name name)
                  (host-command "add-track-instrument" (dict :name name)))
                (status "Drop an instrument, not a folder"))
              (if path
                (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
                (status "Drop a sample file, not a folder")))))))))

;; Drag a track badge onto a group container -> add it to that group.
(def mixer-v2-drop-track-into-group (event gidx)
  (let ((trk (get (get event :payload) :track)))
    (do
      (mixer-v2-clear-delete-target)
      (if (>= trk 0)
        (host-command "move-track-to-group" (dict :track trk :gidx gidx))
        false))))

;; Drag a track badge onto the "Drop samples here" zone -> remove it from its group.
(def mixer-v2-drop-track-out-of-group (event)
  (let ((trk (get (get event :payload) :track)))
    (do
      (mixer-v2-clear-delete-target)
      (if (>= trk 0)
        (host-command "remove-track-from-group" (dict :track trk))
        false))))

(def mixer-v2-drop-effect-on-track (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((kind (get payload :kind))
        (name (get payload :name))
        (track (get target :track)))
      (do
        (mixer-v2-select-track track)
        (if (= kind "builtin-audio-effect")
          (host-command "add-builtin-effect-to-track" (dict :track track :name name))
          (if (= kind "custom-audio-effect")
            (host-command "add-effect-to-track" (dict :track track :name name))
            (if (= kind "midi-effect")
              (host-command "add-midi-fx-to-track" (dict :track track :name name))
              (status "Drop an audio or MIDI effect"))))))))

(def mixer-v2-drop-on-track (event)
  (let ((drag-type (get event :drag-type)))
    (if (= drag-type "sound")
      (sbrowser-drop-sound-on-track event)
      (if (= drag-type "instrument")
        (sbrowser-drop-instrument-on-track event)
        (if (= drag-type "sample")
          (mixer-v2-drop-sample-on-track event)
          (if (or (= drag-type "audio-effect") (= drag-type "midi-effect"))
            (mixer-v2-drop-effect-on-track event)
            (status "Unsupported drop")))))))

(def mixer-v2-track-drop-types (i)
  (if (seq-track-replaceable-instrument? i)
    (list "sample" "instrument" "sound" "audio-effect" "midi-effect")
    (if (seq-track-sound-replaceable? i)
      (list "sample" "sound" "audio-effect" "midi-effect")
      (list "sample" "audio-effect" "midi-effect"))))

(def mixer-v2-drop-effect-on-bus (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((kind (get payload :kind))
        (name (get payload :name))
        (bus (get target :bus)))
      (do
        (mixer-v2-select-bus bus)
        (if (= kind "builtin-audio-effect")
          (host-command "add-builtin-bus-effect" (dict :bus bus :name name))
          (if (= kind "custom-audio-effect")
            (host-command "add-bus-effect" (dict :bus bus :name name))
            (status "Drop an audio effect on a bus")))))))

(defstate mixer-v2-pending-mod-source -1)
(def mixer-v2-track-modulator? (i)
  (and (< i (len SEQ.track-instrument-types))
    (= (nth SEQ.track-instrument-types i) "modulator")))

(def mixer-v2-track-mod-output? (i)
  (or (mixer-v2-track-modulator? i)
    (and (< i (len SEQ.track-mod-output-available))
      (nth SEQ.track-mod-output-available i))))

(def mixer-v2-clear-delete-target ()
  (seq-clear-delete-target))

(def mixer-v2-mod-route-dest-kind (route)
  (let ((kind (get route :dest-kind)))
    (if kind kind "track")))

(def mixer-v2-mod-route-exists-at (source dest-kind dest input idx)
  (if (>= idx (len SEQ.mod-routes))
    false
    (let ((route (nth SEQ.mod-routes idx)))
      (or (and (= (get route :source) source)
            (= (mixer-v2-mod-route-dest-kind route) dest-kind)
            (= (get route :dest) dest)
            (= (get route :input) input))
        (mixer-v2-mod-route-exists-at source dest-kind dest input (+ idx 1))))))

(def mixer-v2-mod-route-exists? (source dest input)
  (mixer-v2-mod-route-exists-at source "track" dest input 0))

(def mixer-v2-bus-mod-route-exists? (source bus-id input)
  (mixer-v2-mod-route-exists-at source "bus" bus-id input 0))

(def mixer-v2-mod-route-sources-at (dest-kind dest input idx acc)
  (if (>= idx (len SEQ.mod-routes))
    acc
    (let ((route (nth SEQ.mod-routes idx)))
      (mixer-v2-mod-route-sources-at dest-kind dest input (+ idx 1)
        (if (and (= (mixer-v2-mod-route-dest-kind route) dest-kind)
              (= (get route :dest) dest)
              (= (get route :input) input))
          (append acc (list (get route :source)))
          acc)))))

(def mixer-v2-mod-route-sources (dest input)
  (mixer-v2-mod-route-sources-at "track" dest input 0 (list)))

(def mixer-v2-bus-mod-route-sources (bus-id input)
  (mixer-v2-mod-route-sources-at "bus" bus-id input 0 (list)))

(def mixer-v2-clear-selected-mod-route ()
  (mixer-v2-clear-delete-target))

(def mixer-v2-select-mod-route-kind (source dest-kind dest input)
  (do
    (seq-set-delete-target :mod-route (dict :source source :dest-kind dest-kind :dest dest :input input))
    (status (if (= dest-kind "bus")
      (str "Selected mod route: track " (+ source 1) " out -> group Ext" (+ input 1))
      (str "Selected mod route: track " (+ source 1) " out -> track " (+ dest 1) " Ext" (+ input 1))))))

(def mixer-v2-select-mod-route (source dest input)
  (mixer-v2-select-mod-route-kind source "track" dest input))

(def mixer-v2-selected-mod-sources-at (dest-kind dest input idx acc)
  (if (>= idx (len SEQ.selected-mod-routes))
    acc
    (let ((route (nth SEQ.selected-mod-routes idx)))
      (mixer-v2-selected-mod-sources-at dest-kind dest input (+ idx 1)
        (if (and (= (mixer-v2-mod-route-dest-kind route) dest-kind)
              (= (get route :dest) dest)
              (= (get route :input) input))
          (append acc (list (get route :source)))
          acc)))))

(def mixer-v2-selected-mod-sources (dest input)
  (mixer-v2-selected-mod-sources-at "track" dest input 0 (list)))

(def mixer-v2-selected-bus-mod-sources (bus-id input)
  (mixer-v2-selected-mod-sources-at "bus" bus-id input 0 (list)))

(def mixer-v2-mod-out-click (track)
  (if (mixer-v2-track-mod-output? track)
    (do
      (set! mixer-v2-pending-mod-source track)
      (mixer-v2-clear-delete-target)
      (status (str "Mod out: track " (+ track 1))))
    (do
      (mixer-v2-clear-delete-target)
      (status "This track has no mod output"))))

(def mixer-v2-cancel-mod-draw ()
  (do
    (set! mixer-v2-pending-mod-source -1)
    true))

(def mixer-v2-connect-mod-route (source track input)
  (do
    (mixer-v2-clear-delete-target)
    (if (= source track)
      (status "Mod self-routes are not allowed")
      (if (mixer-v2-mod-route-exists? source track input)
        (status "Mod route already connected")
        (host-command "set-mod-route"
          (dict :source source :dest-kind "track" :dest track :input input))))))

(def mixer-v2-connect-bus-mod-route (source bus-id input)
  (do
    (mixer-v2-clear-delete-target)
    (if (mixer-v2-bus-mod-route-exists? source bus-id input)
      (status "Mod route already connected")
      (host-command "set-mod-route"
        (dict :source source :dest-kind "bus" :dest bus-id :input input)))))

(def mixer-v2-mod-in-click (track input)
  (if (< mixer-v2-pending-mod-source 0)
    false
    (do
      (mixer-v2-connect-mod-route mixer-v2-pending-mod-source track input)
      (set! mixer-v2-pending-mod-source -1))))

(def mixer-v2-bus-mod-in-click (bus-id input)
  (if (< mixer-v2-pending-mod-source 0)
    false
    (do
      (mixer-v2-connect-bus-mod-route mixer-v2-pending-mod-source bus-id input)
      (set! mixer-v2-pending-mod-source -1))))

(defwidget mixer-v2-mod-port
  :width 1.55 :height 1.55
  :paint-margin 0.12
  :state (active pending output selected)
  :shader
  (let ((outer (if active
          (if selected
            (rgba 1.0 0.18 0.12 1.0)
            (if output
              (if pending (rgba 0.48 0.86 1.0 1.0) (rgba 0.10 0.58 1.0 1.0))
              (rgba 1.0 0.52 0.16 1.0)))
          (rgba 0.10 0.11 0.13 1.0)))
      (inner (if active
          (if output (rgba 0.015 0.035 0.055 1.0) (rgba 0.035 0.018 0.006 1.0))
          (rgba 0.02 0.025 0.03 1.0))))
    (sdf/layer
      (sdf/fill (sdf/circle 0.82)
        (material :color outer))
      (sdf/fill (sdf/circle 0.43)
        (material :color inner)))))

(defwidget track-pattern-cell-bg
  :width 0.88 :height 0.38
  :paint-margin 0.04
  :state (active assigned override selected track-r track-g track-b)
  :bindable (active assigned override selected track-r track-g track-b)
  :shader
  (let ((track-col (rgba track-r track-g track-b 1.0))
      (track-r (if (= active 1) (* 1.10 track-r) track-r))
      (track-g (if (= active 1) (* 1.1 track-g) track-g))
      (track-b (if (= active 1) (* 1.1 track-b) track-b))
      (outer (if (= selected 1)
          (rgba 0.94 0.96 1.0 1.0)
          (rgba track-r track-g track-b 1.0)))
      (middle (rgba track-r track-g track-b 1.0))
      (inner (if (= active 0)
          (rgba 0.02 0.025 0.03 0.7)
          (rgba 0.1 0.1 0.1 0.3)
          )))
    ;; The play triangle moved into the sound-glyph shader (it must sit ON TOP
    ;; of the glyph; this background always draws underneath it). Liveness
    ;; comes from the host play-key store (sync_pattern_cell_glyph_frames),
    ;; and patch-less patterns still get an empty glyph frame published, so
    ;; every active cell renders the glyph-drawn triangle — no bg fallback.
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.3)
        (material :color outer))
      (sdf/fill (sdf/rounded-rect (* width 0.94) (* height 0.94) 0.2)
        (material :color middle))
      (sdf/fill (sdf/rounded-rect (* width (if (= active 1) 0.8 0.92))
          (* (if (= active 1) 0.8 0.92) height) (if (= active 1) 0.1 0.22))
        (material :color inner)))))

;; `track-pattern-cell-bg` for a cell whose quantized launch is pending:
;; identical geometry (the sound glyph on top is untouched), but the ring
;; blinks the track color toward white at the queued-scene-pill cadence
;; until the boundary launch fires and the host swaps the background back.
(defwidget track-pattern-cell-queued-bg
  :width 0.88 :height 0.38
  :paint-margin 0.04
  :state (active assigned override selected track-r track-g track-b)
  :bindable (active assigned override selected track-r track-g track-b)
  :animates true
  :shader
  (let ((pulse (+ 0.5 (* 0.5 (cos (* itime 5.4)))))
      (blink-r (+ track-r (* pulse (- 1.0 track-r))))
      (blink-g (+ track-g (* pulse (- 1.0 track-g))))
      (blink-b (+ track-b (* pulse (- 1.0 track-b))))
      (outer (rgba blink-r blink-g blink-b 1.0))
      (middle (rgba blink-r blink-g blink-b 1.0))
      (inner (if (= active 0)
          (rgba 0.02 0.025 0.03 0.7)
          (rgba 0.1 0.1 0.1 0.3))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.3)
        (material :color outer))
      (sdf/fill (sdf/rounded-rect (* width 0.94) (* height 0.94) 0.2)
        (material :color middle))
      (sdf/fill (sdf/rounded-rect (* width (if (= active 1) 0.8 0.92))
          (* (if (= active 1) 0.8 0.92) height) (if (= active 1) 0.1 0.22))
        (material :color inner)))))



(def mixer-v2-track-pattern-cells (track)
  (if (< track (len SEQ.track-pattern-cells))
    (nth SEQ.track-pattern-cells track)
    (list)))

;; The pattern id this track has queued behind a quantized clip launch
;; (-1 = none), from the host's pending-launch poll.
(def mixer-v2-queued-clip (track)
  (let ((queued (or SEQ.queued-track-clips (list))))
    (if (< track (len queued))
      (nth queued track)
      -1)))

(def mixer-v2-launch-track-pattern (track cell)
  (do
    (mixer-v2-activate-track-control track)
    (seq-set-track track)
    ;; Clip launches follow the transport's scene launch quantize: the host
    ;; assigns the cell now and defers the audible launch to the boundary.
    (host-command "set-scene-cell"
      (dict
        :scene (or SEQ.current-pattern 0)
        :track track
        :pattern-id (get cell :id)
        :quantize (or SEQ.scene-launch-quantize "off")))
    (seq-set-delete-target :track-pattern (dict :track track :pattern-id (get cell :id)))))

(def mixer-v2-track-pattern-grid (track)
  (let ((cells (mixer-v2-track-pattern-cells track)))
    (box :width :fill :height 4.0 :align :top :bg :black :background-color :buffer-bg
      (grid :cols 6 :col-width 2.0 :row-height 1.0 :align :center
        (each cells |cell cell-idx|
          (let ((pattern-id (get cell :id)))
            (box
              :key (str "mixer-v2-track-pattern-cell-" track "-" pattern-id)
              :width 1.90 :height 0.95
              :padding 0.35
              :bg :transparent
              :background (if (= pattern-id (mixer-v2-queued-clip track))
                "track-pattern-cell-queued-bg"
                "track-pattern-cell-bg")
              :active (mixer-v2-track-pattern-cell-active-binding track pattern-id)
              :assigned (mixer-v2-track-pattern-cell-assigned-binding track pattern-id)
              :override (mixer-v2-track-pattern-cell-override-binding track pattern-id)
              :selected (mixer-v2-track-pattern-cell-selected-binding track pattern-id)
              ;; Dimmed so the sound glyph on top carries the cell's identity;
              ;; the launch/selection states still read through the shader.
              :track-r (* 0.95 (mixer-v2-track-color-r track false))
              :track-g (* 0.95 (mixer-v2-track-color-g track false))
              :track-b (* 0.95 (mixer-v2-track-color-b track false))
              :on-click (lambda (event) (mixer-v2-launch-track-pattern track cell))
              ;; The pattern's bound sound, as its palette glyph (host feed:
              ;; sync_pattern_cell_glyph_frames). The tuned shader styling is
              ;; the widget default (TUNING_PROPS); the substrate body tints
              ;; with the track color so cells keep their track identity.
              (sound-glyph
                :key (str "mixer-v2-cell-glyph-" track "-" pattern-id)
                :source (str "pattern-glyph:track:" track ":pattern:" pattern-id)
                ;; Quantize to a coarse virtual-pixel grid: the mixer shows ~50
                ;; glyphs at once, so the palette's hi-def gooey rendering reads
                ;; as noise at this size. Odd count keeps a cell centered.
                :pixelate 2
                :edge-soft 0.1
                :white-damp 0
                :height-in 0.3
                :height-out -0.08
                :height-amp 3
                :diffuse 0.8
                :rim-width 0.1
                ;; Leave the active launch mark visually dominant: its host-
                ;; driven play state shrinks and dims only the identity glyph.
                :play-glyph-padding 0.14
                :play-glyph-opacity 0.4
                :play-color :white
                :tint-r (* 0.4 (mixer-v2-track-color-r track false))
                :tint-g (* 0.4 (mixer-v2-track-color-g track false))
                :tint-b (* 0.4 (mixer-v2-track-color-b track false))
                ))))))))

(def mixer-v2-mod-output-style
  (ui/style
    :hover (dict
      :brightness 1.45
      :transition (dict :brightness 0.08 :ease :smoothstep))))

(def mixer-v2-mod-port-row (track)
     (box :height 0.8 :width :fill 
  (h-stack :key (str "mixer-v2-mod-ports-" track)
    :width 9.8 :height 0.1 :gap 0.42 :align :center
    (mixer-v2-mod-port
      :key (str "mixer-v2-mod-out-" track)
      :patch-port true
      :direction :out
      :track track
      :active (mixer-v2-track-mod-output? track)
      :pending (= mixer-v2-pending-mod-source track)
      :output true
      :selected false
      :style (if (mixer-v2-track-mod-output? track) mixer-v2-mod-output-style nil)
      :on-click |x y r| (mixer-v2-mod-out-click track)
      :on-mouse-down |x y r| (mixer-v2-mod-out-click track)
      :on-patch-cancel (lambda (source)
        (mixer-v2-cancel-mod-draw))
      :on-patch-miss (lambda ()
        (mixer-v2-clear-selected-mod-route)))
    (if (mixer-v2-track-modulator? track)
      (each (range 0 4) |input|
        (box :key (str "mixer-v2-mod-in-spacer-" track "-" input)
          :width 1.05 :height 1.05))
      (each (range 0 4) |input|
        (mixer-v2-mod-port
          :key (str "mixer-v2-mod-in-" track "-" input)
          :patch-port true
          :direction :in
          :track track
          :input input
          :connected-sources (mixer-v2-mod-route-sources track input)
          :selected-sources (mixer-v2-selected-mod-sources track input)
          :active true
          :pending false
          :output false
          :selected (> (len (mixer-v2-selected-mod-sources track input)) 0)
          :on-patch-drop (lambda (source dest input)
            (do
              (mixer-v2-connect-mod-route source dest input)
              (set! mixer-v2-pending-mod-source -1)))
          :on-cable-click (lambda (source dest input)
            (mixer-v2-select-mod-route source dest input))
          :on-click |x y r| (mixer-v2-mod-in-click track input)
          :on-mouse-up |x y r| (mixer-v2-mod-in-click track input)))))))

(def mixer-v2-bus-mod-port-row (bus-id)
  (box :height 0.8 :width :fill
    (h-stack :key (str "mixer-v2-bus-mod-ports-" bus-id)
      :width 7.1 :height 0.1 :gap 0.42 :align :center
      (each (range 0 4) |input|
        (mixer-v2-mod-port
          :key (str "mixer-v2-bus-mod-in-" bus-id "-" input)
          :patch-port true
          :direction :in
          :dest-kind "bus"
          :dest bus-id
          :input input
          :connected-sources (mixer-v2-bus-mod-route-sources bus-id input)
          :selected-sources (mixer-v2-selected-bus-mod-sources bus-id input)
          :active true
          :pending false
          :output false
          :selected (> (len (mixer-v2-selected-bus-mod-sources bus-id input)) 0)
          :on-patch-drop (lambda (source dest input)
            (do
              (mixer-v2-connect-bus-mod-route source bus-id input)
              (set! mixer-v2-pending-mod-source -1)))
          :on-cable-click (lambda (source dest input)
            (mixer-v2-select-mod-route-kind source "bus" bus-id input))
          :on-click |x y r| (mixer-v2-bus-mod-in-click bus-id input)
          :on-mouse-up |x y r| (mixer-v2-bus-mod-in-click bus-id input))))))

(defwidget mixer-v2-volume-triangle
  :width 1.35 :height 4.24
  :paint-margin 0.15
  :state (value)
  :bindable (value)
  :shader
  (let ((marker-y (* (- 1.0 (* 2.0 value)) (* height 1.04))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.04)
        (material :color (rgba 0 0 0 0)))
      (sdf/fill
        (sdf/translate 0 marker-y
          (let ((tx x)
                (ty (* y aspect))
                (p1x -0.82) (p1y -0.15) (p2x -0.82) (p2y 0.15) (p3x 0.70) (p3y 0.0))
            (let ((d1 (- (* (- p2x p1x) (- ty p1y)) (* (- p2y p1y) (- tx p1x))))
                  (d2 (- (* (- p3x p2x) (- ty p2y)) (* (- p3y p2y) (- tx p2x))))
                  (d3 (- (* (- p1x p3x) (- ty p3y)) (* (- p1y p3y) (- tx p3x)))))
              (max (max d1 d2) d3))))
        (material :color (rgba 0.78 0.80 0.83 1.0))))))

(def mixer-v2-level-color (level)
  (if (> level 0.88)
    (rgba 0.95 0.18 0.16 1.0)
    (if (> level 0.70)
      (rgba 0.96 0.82 0.18 1.0)
      (rgba 0.10 0.85 0.30 1.0))))

(def mixer-v2-meter (level-l level-r)
  (mixer-meter
    :level-l level-l :level-r level-r
    :width 2.22 :height 4.24
    :font-size 7 :label-height 0.42 :label-top-inset 0.0
    :label-color :dim))

(def mixer-v2-track-meter (i)
  (subtree :key (str "mixer-v2-track-meter-" i)
    (mixer-v2-meter (track-peak i) (track-peak i))))

(def mixer-v2-bus-meter (i)
  (subtree :key (str "mixer-v2-bus-meter-" i)
    (mixer-v2-meter (bus-peak-l i) (bus-peak-r i))))

(def mixer-v2-track-meter-control (i)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event)
      (do
        (mixer-v2-clear-delete-target)
        (seq-set-track-volume i (mixer-v2-event-volume event))))
    :on-drag (lambda (event)
      (do
        (mixer-v2-clear-delete-target)
        (seq-set-track-volume i (mixer-v2-event-volume event))))
    (h-stack :gap 0.06 :align :center
      (mixer-v2-volume-triangle
        :value (bind-seq (mixer-v2-track-volume-field i))
        :on-click (lambda (sx sy region)
          (do
            (mixer-v2-clear-delete-target)
            (seq-set-track-volume i (mixer-v2-pointer-volume sy))))
        :on-drag (lambda (sx sy region)
          (do
            (mixer-v2-clear-delete-target)
            (seq-set-track-volume i (mixer-v2-pointer-volume sy)))))
      (mixer-v2-track-meter i))))

(def mixer-v2-bus-meter-control (i)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event)
      (do
        (mixer-v2-select-bus i)
        ;(seq-set-bus-volume i (mixer-v2-event-volume event))
        )
      )
    :on-drag (lambda (event)
      (do
        (mixer-v2-select-bus i)
        ;;(seq-set-bus-volume i (mixer-v2-event-volume event))
        ))
    (v-stack
      (box :width :fill :height 6.5)
      (h-stack :gap 0.06 
        (box :width 2 )
        (mixer-v2-volume-triangle
          :value (bind-seq-nth "bus-volumes" i)
          :on-click (lambda (sx sy region)
            (do
              (mixer-v2-select-bus i)
              (seq-set-bus-volume i (mixer-v2-pointer-volume sy))))
          :on-drag (lambda (sx sy region)
            (do
              (mixer-v2-select-bus i)
              (seq-set-bus-volume i (mixer-v2-pointer-volume sy)))))
        (mixer-v2-bus-meter i)))))

(def mixer-v2-send-label (name)
  (if (= name "Bus A")
    "A"
    (if (= name "Bus B")
      "B"
      (substring name 0 3))))

(def mixer-v2-send-field (track bus)
  (str "track-" track "-bus-" bus "-send"))

(def mixer-v2-track-volume-field (track)
  (str "track-" track "-volume"))

(def mixer-v2-track-pan-field (track)
  (str "track-" track "-pan"))

(def mixer-v2-send-knob (track send)
  (knob-number :label (mixer-v2-send-label (get send :name))
    :key (str "mixer-v2-track-" track "-send-" (get send :bus-idx))
    :value (bind-seq (mixer-v2-send-field track (get send :bus-idx)))
    :min 0 :max 1 :decimals 2
    :show-value false
    :font-size 9 :label-font-size 5
    :text-color :dim :label-color :dim
    :width 3.4  :height 2.0 :knob-size 1.44
    :on-change (lambda (v)
      (do
        (mixer-v2-clear-delete-target)
        (host-command "set-track-bus-send"
          (dict :track track :bus (get send :bus-idx) :amount v))))))

(def mixer-v2-track-strip (i)
  (let ((sends (nth SEQ.track-bus-sends i)))
    ;; Grouped strips drop the output dropdown, so they are shorter to avoid
    ;; dead space at the bottom inside the group container.
    ;; Mute/name/output reads live in bindings or nested subtrees so those
    ;; changes don't rerun the whole strip.
    (box :width 12.9 :height (if (mixer-v2-track-grouped? i) 13.5 13.8)
      :selected (mixer-v2-track-selected-binding i)
      :muted (bind-seq-nth "track-muted-effective" i)
      :background-color :mixer-strip-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 2
      :corner-radius 10
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :muted-border-color :mixer-strip-border
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (mixer-v2-track-drop-types i)
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (mixer-v2-drop-on-track event))
      :on-click (lambda (event) (mixer-v2-track-body-click event i))
      (v-stack :gap 0.20
        ;; Grouped tracks drop the output dropdown (their output is the group
        ;; bus); the container provides the color above. A small spacer keeps
        ;; the pattern grid aligned with loose strips.
        (if (mixer-v2-track-grouped? i)
          (box :width :fill :height 0.85 :bg :transparent)
          (subtree :key (str "mixer-v2-track-output-sub-" i)
            (dropdown :value (nth SEQ.track-outputs i)
              :key (str "mixer-v2-track-output-" i)
              :options SEQ.track-output-options
              :on-change (lambda (v)
                (do
                  (mixer-v2-clear-delete-target)
                  (host-command "set-track-output" (dict :track i :label v))))
              :width :fill :height 1.2 :font-size 10)))
        (mixer-v2-track-pattern-grid i)
        
        	       
        (h-stack :gap 1.6 :align :center
          (v-stack
            
            (h-stack :gap 0.05
              ;; Only the first two send knobs (Bus A / Bus B); group-backing
              ;; bus sends are not shown on the strip.
              (each (range 0 (min 2 (len sends))) |send-idx|
                (mixer-v2-send-knob i (nth sends send-idx))))
            
            (knob-number :label "pan"
              :key (str "mixer-v2-track-pan-" i)
              :value (bind-seq (mixer-v2-track-pan-field i))
              :min -1 :max 1 :origin 0 :decimals 2
              :font-size 9 :label-font-size 8
              :text-color :dim :label-color :dim
              :width 6.5 :height 2.35 :knob-size 2.58
              :on-change (lambda (v)
                (do
                  (mixer-v2-clear-delete-target)
                  (seq-set-track-pan i v)))))
          
          (mixer-v2-track-meter-control i))
        
        (box :width :fill :height 0.05)
        (subtree :key (str "mixer-v2-strip-buttons-" i)
          (mixer-v2-strip-buttons i))
        (mixer-v2-mod-port-row i)
        (subtree :key (str "mixer-v2-strip-label-" i)
          (mixer-v2-strip-label i))))))

;; Mute/solo/arm buttons in their own subtree: mute/solo/arm changes rerun
;; just this row instead of the whole strip.
(def mixer-v2-strip-buttons (i)
  (let ((muted (mixer-v2-muted? i)))
    (h-stack :gap 0.35
      (button (str (+ i 1))
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :background-color (if muted :mixer-control-bg (rgba 0.95 0.48 0.18 1.0))
        :color (if muted :dim :black)
        :on-click (lambda (event) (do (mixer-v2-activate-track-control i) (seq-toggle-track-mute i))))
      (button "S"
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :background-color (mixer-v2-button-bg (nth SEQ.track-solos i))
        :color (if (nth SEQ.track-solos i) :black :dim)
        :on-click (lambda (event) (do (mixer-v2-activate-track-control i) (seq-toggle-track-solo i))))
      (button "R"
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :background-color (mixer-v2-arm-bg (nth SEQ.record-armed i))
        :color (if (nth SEQ.record-armed i) :black :dim)
        :on-click (lambda (event) (do (mixer-v2-activate-track-control i) (seq-toggle-record-arm i)))))))

;; Name label in its own subtree: rename/mute changes rerun just the label.
(def mixer-v2-strip-label (i)
  (let ((muted (mixer-v2-muted? i)))
    (box
      :key (str "mixer-v2-track-label-" i)
      :width :fill :height 1.0
      :corner-radius 30
      :padding 0
      :drag-type "track-badge"
      :drag-payload (dict :track i)
      :selected (mixer-v2-track-delete-target-binding i)
      :background-color (rgba
        (mixer-v2-track-color-r i muted)
        (mixer-v2-track-color-g i muted)
        (mixer-v2-track-color-b i muted)
        1.0)
      :selected-background-color :fx-panel-header-selected-bg
      :on-click (lambda (event) (mixer-v2-track-label-click event i))
      :on-double-click (lambda (event) (seqv-open-piano-roll-for-track i))
      (badge (substring (nth SEQ.track-names i) 0 10)
        :key (str "mixer-v2-track-label-content-" i)
        :icon (seq-track-type-icon i)
        :width 9.8
        :height 1.0
        :padding 0
        :font-size 10
        :h-align :center
        :background-color :transparent
        :border-color :transparent
        :highlight-color :transparent
        :shadow-color :transparent
        :color (if muted :dim :black)
        :active (mixer-v2-track-delete-target-binding i)
        :active-color :white
        :bg :transparent))))

(def mixer-v2-track-collapsed-label (i)
  (str (+ i 1) " " (substring (nth SEQ.track-names i) 0 3)))

(def mixer-v2-track-collapsed-strip (i)
  (let ((muted (mixer-v2-muted? i)))
    (box :width 4.7 :height 12.15
      :selected (mixer-v2-track-selected-binding i)
      :muted muted
      :background-color :mixer-strip-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 2
      :corner-radius 10
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :muted-border-color :mixer-strip-border
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (mixer-v2-track-drop-types i)
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (mixer-v2-drop-on-track event))
      :on-click (lambda (event) (mixer-v2-track-body-click event i))
      (v-stack :gap 0.42 :align :center
        (box :width :fill :height 3.45 :bg :transparent)
        (mixer-v2-track-meter-control i)
        (button "M"
          :key (str "mixer-v2-track-collapsed-mute-" i)
          :width 3.65 :height 1.0 :padding 0 :font-size 10
          :background-color (mixer-v2-button-bg (nth SEQ.track-mutes i))
          :color (if (nth SEQ.track-mutes i) :black :dim)
          :on-click (lambda (event) (do (mixer-v2-activate-track-control i) (seq-toggle-track-mute i))))
        (box
          :key (str "mixer-v2-track-collapsed-label-" i)
          :width 3.65 :height 1.0
          :padding 0
          :selected (mixer-v2-track-delete-target-binding i)
          :background-color (rgba
            (mixer-v2-track-color-r i muted)
            (mixer-v2-track-color-g i muted)
            (mixer-v2-track-color-b i muted)
            1.0)
          :selected-background-color :fx-panel-header-selected-bg
          :on-click (lambda (event) (mixer-v2-track-label-click event i))
          :on-double-click (lambda (event) (seqv-open-piano-roll-for-track i))
          (badge (mixer-v2-track-collapsed-label i)
            :key (str "mixer-v2-track-collapsed-label-content-" i)
            :icon (seq-track-type-icon i)
            :width 3.65
            :height 1.0
            :padding 0
            :font-size 9
            :h-align :center
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :color (if muted :dim :black)
            :active (mixer-v2-track-delete-target-binding i)
            :active-color :white
            :bg :transparent))))))

(def mixer-v2-bus-label (i)
  (if (= i 0) "Main" (nth SEQ.bus-names i)))

(def mixer-v2-bus-mute-label (i)
  (if (= i 0) "M" (if (= i 1) "A" (if (= i 2) "B" (str i)))))

(def mixer-v2-has-mix-bus? ()
  (and (> (len SEQ.bus-names) 0) (= (nth SEQ.bus-names 0) "Mix")))

(def mixer-v2-display-bus-index (display-i)
  (if (or (not (mixer-v2-has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    display-i
    (if (= display-i (- (len SEQ.bus-names) 1))
      0
      (+ display-i 1))))

(def mixer-v2-bus-display-index (bus-i)
  (if (or (not (mixer-v2-has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    bus-i
    (if (= bus-i 0)
      (- (len SEQ.bus-names) 1)
      (- bus-i 1))))

(def mixer-v2-channel-count ()
  (+ SEQ.num-tracks (len SEQ.bus-names)))

(def mixer-v2-current-channel-index ()
  (if (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names)))
    (+ SEQ.num-tracks (mixer-v2-bus-display-index selected-bus))
    (let ((selected (filter
            (lambda (i) (> (reactive-value (bind-seq (str "track-selected-" i))) 0.5))
            (range 0 SEQ.num-tracks))))
      (if (> (len selected) 0)
        (nth selected 0)
        0))))

(def mixer-v2-select-channel-index (idx)
  (let ((clamped (min (max idx 0) (- (max (mixer-v2-channel-count) 1) 1))))
    (if (< clamped SEQ.num-tracks)
      (do
        (set! selected-bus -1)
        (mixer-v2-clear-delete-target)
        (seq-set-track clamped))
      (do
        (mixer-v2-clear-delete-target)
        (seq-clear-selection)
        (set! selected-bus (mixer-v2-display-bus-index (- clamped SEQ.num-tracks)))))))

(def mixer-v2-select-prev-channel ()
  (do
    (mixer-v2-select-channel-index (- (mixer-v2-current-channel-index) 1))
    true))

(def mixer-v2-select-next-channel ()
  (do
    (mixer-v2-select-channel-index (+ (mixer-v2-current-channel-index) 1))
    true))

(def mixer-v2-delete-selected-track ()
  (seq-delete-active-target))

(def mixer-v2-handle-key (key text)
  (if (= key "LEFT")
    (mixer-v2-select-prev-channel)
    (if (= key "RIGHT")
      (mixer-v2-select-next-channel)
      (if (= key "BS")
        (seq-delete-active-target)
        (if (= key "Delete")
          (seq-delete-active-target)
          false)))))

(def mixer-v2-bus-strip (i)
  (let ((selected (= selected-bus i)))
    (box :key (str "mixer-v2-bus-strip-" i)
      :width 10.3 :height 13.8
      :background-color (mixer-v2-strip-bg selected (nth SEQ.bus-mutes i))
      :border-width 2
      :corner-radius 10
      :border-color (mixer-v2-strip-border selected)
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (list "audio-effect")
      :drop-meta (dict :kind "bus" :bus i)
      :on-drop (lambda (event) (mixer-v2-drop-effect-on-bus event))
      :on-click (lambda (event) (mixer-v2-select-bus i))
      (v-stack :gap 0.25
        (box :height 1.55)
        ;; Mix/Main is the graph output and has no external modulation inputs.
        ;; Every other bus has the same four backend inputs used by group buses.
        ;(if (= (nth SEQ.bus-names i) "Mix")
        ;  (box :height 0.8 :width :fill :bg :transparent)
        ;  )
        (h-stack :gap 0.45 :align :center
          (box :width 3.0 :height 5.0)
          (mixer-v2-bus-meter-control i))
        (box :height 2.8)
        (h-stack :gap 0.35
          (button (mixer-v2-bus-mute-label i)
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (if (nth SEQ.bus-mutes i) :mixer-control-bg (rgba 0.95 0.48 0.18 1.0))
            :border-color :transparent
            :color (if (nth SEQ.bus-mutes i) :dim :black)
            :on-click (lambda (event) (do (mixer-v2-select-bus i) (seq-toggle-bus-mute i))))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-button-bg (nth SEQ.bus-solos i))
            :border-color :transparent
            :color (if (nth SEQ.bus-solos i) :black :dim)
            :on-click (lambda (event) (do (mixer-v2-select-bus i) (seq-toggle-bus-solo i))))
          (box :width 2.1 :height 1.0))
        (if (not (= (nth SEQ.bus-names i) "Mix"))
          (mixer-v2-bus-mod-port-row (nth SEQ.bus-ids i))
          (box :height 0.8)
          )
        
        (button (mixer-v2-bus-label i)
          :width :fill :height 1.0 :padding 0 :font-size 10
          :background-color :mixer-label-bg
          :border-color :transparent
          :color :white
          :on-click (lambda (event) (mixer-v2-select-bus i)))))))

;; --- Track groups -------------------------------------------------------

(def mixer-v2-list-contains? (xs v)
  (> (len (filter (lambda (x) (= x v)) xs)) 0))

;; Index (in SEQ.groups) of the group anchored at track i (lowest member), else -1.
(def mixer-v2-group-anchored-at (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (= (get (nth SEQ.groups gidx) :anchor) i) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

;; Index (in SEQ.groups) of the group containing track i, else -1.
(def mixer-v2-group-of-track (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (mixer-v2-list-contains? (get (nth SEQ.groups gidx) :members) i) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

(def mixer-v2-track-grouped? (i)
  (>= (mixer-v2-group-of-track i) 0))

(def mixer-v2-group-color (gidx)
  (let ((c (get (nth SEQ.groups gidx) :color)))
    (if (>= (len c) 3) c (list 0.5 0.5 0.5))))

;; Storage index of the bus with the given id, or -1.
(def mixer-v2-bus-index-by-id (bid)
  (reduce |acc i|
    (if (>= acc 0)
      acc
      (if (= (nth SEQ.bus-ids i) bid) i acc))
    -1
    (range 0 (len SEQ.bus-ids))))

;; Set of bus ids that back a group (hidden from the ordinary bus list).
(def mixer-v2-group-bus-id? (bid)
  (reduce |acc gidx|
    (or acc (= (get (nth SEQ.groups gidx) :bus-id) bid))
    false
    (range 0 (len SEQ.groups))))

;; Build the flat mixer render-item list: loose tracks and group containers,
;; each group anchored at its lowest member index (visual contiguity without
;; reindexing the track Vec).
(def mixer-v2-render-order ()
  (reduce |acc i|
    (let ((ganch (mixer-v2-group-anchored-at i)))
      (if (>= ganch 0)
        (append acc (list (dict :kind "group" :gidx ganch)))
        (if (mixer-v2-track-grouped? i)
          acc
          (append acc (list (dict :kind "loose" :track i))))))
    (list)
    (range 0 SEQ.num-tracks)))

(def mixer-v2-toggle-group-collapsed (gid)
  (seq-toggle-group-collapsed gid))

(def mixer-v2-select-group (gidx)
  (let ((idx (mixer-v2-bus-index-by-id (get (nth SEQ.groups gidx) :bus-id))))
    (if (>= idx 0)
      (mixer-v2-select-bus idx)
      false)))

;; True when this group's backing bus is the currently selected channel.
(def mixer-v2-group-selected? (gidx)
  (let ((idx (mixer-v2-bus-index-by-id (get (nth SEQ.groups gidx) :bus-id))))
    (and (>= idx 0) (= selected-bus idx))))

;; The group's own channel slot (collapse toggle + name) shown at the left of
;; the container, over the container color.
(def mixer-v2-group-header-slot (gidx)
  (let ((group (nth SEQ.groups gidx))
      (c (mixer-v2-group-color gidx))
      (bus-idx (mixer-v2-bus-index-by-id (get (nth SEQ.groups gidx) :bus-id))))
    (box :key (str "mixer-v2-group-bus-strip-" bus-idx)
      :width 9.0 :height 13.72
      :corner-radius 12
      :padding 0.0
      :background-color :mixer-strip-bg
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (if (>= bus-idx 0)
        (list "sample" "instrument" "audio-effect")
        (list))
      :drop-meta (dict :kind "bus" :bus bus-idx)
      :on-drop (lambda (event) (mixer-v2-drop-on-group-header event gidx))
      (v-stack :gap 0.4 :align :center
        
        ;; Meter + fader reflect the group's backing bus. Selecting/dragging
        ;; them selects the group's bus (mixer-v2-bus-meter-control selects by
        ;; index). Fall back to nothing if the bus can't be resolved.
        (if (>= bus-idx 0)
          (box  :width :fill :height 9.55
            (mixer-v2-bus-meter-control bus-idx))
          (box :width 0.0 :height 0.0 :bg :transparent))
        (box :height 0.95)
        (mixer-v2-bus-mod-port-row (get group :bus-id))
        (box :corner-radius 16 :background-color c :width 8.5 :padding 0.2
          :key (str "mixer-v2-group-badge-" (get group :id))
          :on-click (lambda (event) (mixer-v2-select-group gidx))
          (h-stack :gap 0.2
            (button (if (get group :collapsed) "▸" "▾")
              :width 2.0 :height 0.9 :padding 0 :font-size 14
              :background-color '(rgba 0.1 0.1 0.1 0.5)
              :border-color '(rgba 0.8 0.8 0.8 0.9)
              :color :white
              :on-click (lambda (event)
                (do
                  (mixer-v2-select-group gidx)
                  (mixer-v2-toggle-group-collapsed (get group :id)))))
            (label (substring (get group :name) 0 10)
              :font-size 11
              :h-align :center
              :color :black
              :bg :transparent))        )))))

(def mixer-v2-drop-new-track-into-group (event gidx)
  (let ((payload (get event :payload))
      (group (nth SEQ.groups gidx)))
    (let ((path (get payload :path))
        (name (get payload :name))
        (group-id (get group :id)))
      (do
        (mixer-v2-select-group gidx)
        (if (= (get event :drag-type) "instrument")
          (if name
            (do
              (set! sbrowser-loading-instrument-name name)
              (host-command "add-track-instrument" (dict :name name :group-id group-id)))
            (status "Drop an instrument, not a folder"))
          (if path
            (host-command "add-track-sample"
              (dict :path path :group-id group-id :preserve-browser-context true))
            (status "Drop a sample file, not a folder")))))))

(def mixer-v2-drop-on-group-header (event gidx)
  (if (= (get event :drag-type) "audio-effect")
    (mixer-v2-drop-effect-on-bus event)
    (mixer-v2-drop-new-track-into-group event gidx)))

(def mixer-v2-group-member-strip (i)
  (if (seq-track-collapsed? i)
    (mixer-v2-track-collapsed-strip i)
    (mixer-v2-track-strip i)))

;; A group rendered as a real container: a colored box wrapping the group's
;; channel slot and its member strips, with a top spacer so the color shows
;; above the contained tracks.
(def mixer-v2-group-container (gidx)
  (let ((group (nth SEQ.groups gidx))
      (c (mixer-v2-group-color gidx))
      (selected (mixer-v2-group-selected? gidx)))
    (box
      :corner-radius 12
      :padding 0.2
      :background-color (rgba (nth c 0) (nth c 1) (nth c 2) (if selected 1.0 0.78))
      :border-width (if selected 4 2)
      :border-color (if selected :mixer-strip-selected-border :mixer-strip-border)
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (list "track-badge")
      :drop-meta (dict :kind "group" :gidx gidx)
      :on-drop (lambda (event) (mixer-v2-drop-track-into-group event gidx))
      :on-click (lambda (event) (mixer-v2-select-group gidx))
      (h-stack :gap 0.0 :align :start
        (mixer-v2-group-header-slot gidx)
        (if (get group :collapsed)
          (box :width 0.0 :height 0.0 :bg :transparent)
          (v-stack :gap 0.0
            (box :width :fill :height 0.2 :bg :transparent)
            (h-stack :gap 0.1
              (each (get group :members) |m|
                (subtree :key (str "mixer-v2-track-" m)
                  (mixer-v2-group-member-strip m))))
            ))))))

(def mixer-v2-render-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "group")
      (let ((gidx (get item :gidx)))
        (subtree :key (str "mixer-v2-group-" (get (nth SEQ.groups gidx) :id))
          (mixer-v2-group-container gidx)))
      (let ((i (get item :track)))
        (subtree :key (str "mixer-v2-track-" i)
          (if (seq-track-collapsed? i)
            (mixer-v2-track-collapsed-strip i)
            (mixer-v2-track-strip i)))))))

(def mixer-v2-sample-drop-zone ()
  (box :key "mixer-v2-sample-drop-zone"
    :width 11.8 :height 13.8
    :background-color :buffer-bg
    :drop-hover-background-color :mixer-control-bg
    :border-width 2
    :border-color :mixer-strip-border
    :drop-hover-border-color :mixer-strip-selected-border
    :corner-radius 10
    :padding 0.5
    :align :center
    :drop-types (list "sample" "instrument" "sound" "track-badge")
    :drop-meta (dict :kind "new-sample-track")
    :on-drop (lambda (event) (mixer-v2-drop-sample-new-track event))
    (label "Drop sounds here"
      :font-size 9.5
      :color :gray
      :bg :transparent)))

(effect-buffer "*mixer*"
  (h-stack :padding 0.2 :gap 0.3
    (each (mixer-v2-render-order) |item|
      (mixer-v2-render-item item))
    (box :width 1.0 :height 11.0)
    (mixer-v2-sample-drop-zone)
    (box :width 1.0 :height 11.0)
    (each (range 0 (len SEQ.bus-names)) |display-i|
      (let ((i (mixer-v2-display-bus-index display-i)))
        (subtree :key (str "mixer-v2-bus-" i)
          (if (mixer-v2-group-bus-id? (nth SEQ.bus-ids i))
            (box :width 0.0 :height 0.0)
            (mixer-v2-bus-strip i)))))))

;; C-g — fold the multi-selected tracks into a new group.
(def mixer-v2-group-selected ()
  (do
    (host-command "group-selected-tracks" (dict))
    true))

;; Global C-g dispatcher: a 2+ track multi-selection only exists via mixer
;; cmd-click, so group when one is present; otherwise open the agent.
(def seq-ctrl-g ()
  (if (>= (len SEQ.selected-tracks) 2)
    (mixer-v2-group-selected)
    (agent-open-instrument)))

(define-mode "seq-mixer-mode" :read-only true :on-key "mixer-v2-handle-key")
(mode-bind-key "seq-mixer-mode" "LEFT" "mixer-v2-select-prev-channel")
(mode-bind-key "seq-mixer-mode" "RIGHT" "mixer-v2-select-next-channel")
(set-buffer-mode-for "*mixer*" "seq-mixer-mode")
