;; ui/audio-clip.lisp -- the audio clip editor (docs/audio-tracks-spec.md
;; §8.2), Ableton's clip view for an audio region: the region's whole source
;; as a waveform, with the part it plays between start and end markers.
;;
;; It shows in the lower panel's *piano-roll* buffer when the edited track
;; is an audio track, so double-clicking a region and the FX <-> editor
;; toggle reach it exactly as they reach the piano roll. The arrangement
;; binds the region (`bind!`); the region's fields come from the host's
;; `track.audio-regions`.

(module eseq.audio-clip)
(import eseq.kinds :refer (track song piano-roll select-region! clear-region!))

(export audio-clip bind! clear! bound-region editor region-label waveform-action
        audio-track? song-region-copy song-region-paste song-region-duplicate
        song-region-key region-copy-key region-paste-key region-duplicate-key focus-key)

;; The bound region (track position and region id, -1 for none), the marker
;; window a drag is previewing (nil when none), the range a body drag selects
;; (source seconds, nil when none), and the waveform's view: visible start and
;; duration in source seconds (0 = the whole source).
(def-kind audio-clip
  :key ()
  :state ((track -1)
          (region -1)
          (pending :any :default nil)
          (range :any :default nil)
          (view-start 0.0)
          (view-duration 0)
          (cursor 0.0)))

(def bind! (i cid)
  (unless (and (= audio-clip.track i) (= audio-clip.region cid))
    (set! audio-clip.view-start 0.0)
    (set! audio-clip.view-duration 0)
    (set! audio-clip.cursor 0.0))
  (set! audio-clip.pending nil)
  (set! audio-clip.range nil)
  (set! audio-clip.track i)
  (set! audio-clip.region cid))

(def clear! ()
  (set! audio-clip.pending nil)
  (set! audio-clip.range nil)
  (set! audio-clip.region -1))

;; The bound region of track t (a map from t.audio-regions), or nil.
(def bound-region (t)
  (when (and t (= t.index audio-clip.track))
    (first (filter |r| (= (get r :cid) audio-clip.region) (or t.audio-regions '())))))

(def region-label (r)
  (str (get r :name) (if (get r :loaded) "" " (offline)")))

;; `text` cut to `n` characters with an ellipsis: labels do not clip.
(def truncate-text (text n)
  (if (> (len text) n) (str (substring text 0 (- n 3)) "...") text))

;; The timeline beat where source second `sec` plays in region r (its first
;; pass), clamped to the region.
(def beat-at-second (r sec)
  (let ((beats-per-second (if (> r.span 0) (/ (- r.end r.start) r.span) 0)))
    (max r.start (min r.end (+ r.start (* (- sec r.offset) beats-per-second))))))

;; A body drag selected [start, end) source seconds: select the same stretch
;; of the timeline as an arrangement region, so Cmd+C / Cmd+D / Backspace act
;; on it (Ableton selects in the clip view and the arrangement together).
(def select-range! (r start end)
  (let ((beat-a (beat-at-second r start))
        (beat-b (beat-at-second r end))
        (t (track audio-clip.track)))
    (if (and t (> beat-b beat-a))
      (do
        (set! audio-clip.range (dict :start start :end end))
        (select-region! t t beat-a beat-b))
      (do
        (set! audio-clip.range nil)
        (clear-region!)))))

;; The marker window — the part of the source the region plays, or its loop
;; while it loops: the drag preview while one is live, else the region's.
(def region-window-start (r)
  (if r.loop r.loop-start r.offset))

(def region-window-end (r)
  (if r.loop r.loop-end (+ r.offset r.span)))

(def window-start (r)
  (let ((p audio-clip.pending))
    (if p (get p :start) (region-window-start r))))

(def window-end (r)
  (let ((p audio-clip.pending))
    (if p (get p :end) (region-window-end r))))

(def set-window! (r start end live)
  (when (> end start)
    (unless (and (= start (region-window-start r)) (= end (region-window-end r)))
      (host-command "audio-region-set-window"
        (dict :id r.cid :offset start :length (- end start) :live live)))))

(def commit-window (r start end)
  (set! audio-clip.pending nil)
  (set-window! r start end false))

(def clamp-view-start (start duration)
  (max 0 (min start (max 0 (- duration audio-clip.view-duration)))))

;; Bar and marker drags preview locally and commit once on release, so a
;; drag is one undo step; a selection made without one commits at once.
(def waveform-action (r event)
  (let ((duration r.duration))
    (match event.type
      :set-cursor
      (do
        (set! audio-clip.cursor event.time)
        (set! audio-clip.range nil))
      ;; A body drag selects a range (never the markers: only the bar moves
      ;; those); the release mirrors it onto the timeline.
      :set-range
      (set! audio-clip.range (dict :start (max 0 event.start) :end (min duration event.end)))
      :end-range
      (if (> event.end event.start)
        (select-range! r (max 0 event.start) (min duration event.end))
        (set! audio-clip.range nil))
      :begin-marker-drag
      (set! audio-clip.pending (dict :start (window-start r) :end (window-end r) :drag true))
      :set-selection
      (let ((start (max 0 event.start))
            (end (min duration event.end)))
        (if (and audio-clip.pending (get audio-clip.pending :drag))
          (set! audio-clip.pending (dict :start start :end end :drag true))
          (commit-window r start end)))
      :end-marker-drag
      (let ((p audio-clip.pending))
        (if p (commit-window r (get p :start) (get p :end)) nil))
      :scroll-view
      (set! audio-clip.view-start
        (clamp-view-start (+ audio-clip.view-start event.delta-time) duration))
      :zoom-view
      (let ((current (if (= audio-clip.view-duration 0) duration audio-clip.view-duration))
            (anchor-ratio (/ (- event.anchor-time audio-clip.view-start) current))
            (next (max 0.01 (min duration (/ current event.factor)))))
        (set! audio-clip.view-duration next)
        (set! audio-clip.view-start
          (clamp-view-start (- event.anchor-time (* anchor-ratio next)) duration)))
      _ nil)))

(def gain-label (db)
  (str (if (> db 0) "+" "") (/ (round (* db 10)) 10) " dB"))

(def pitch-label (st)
  (str (if (> st 0) "+" "") (round st) " st"))

;; ── The clip panel (Ableton's Clip / Audio boxes) ──────────────────────────

(def panel-width 15)

(def section-label (key text)
  (label text :key key :font-size 10 :color :dim :bg :transparent))

;; A seconds field: drag or type; a drag is one undo step.
(def seconds-field (key value lo hi on-change)
  (number-picker :key key :width 6.6 :height 1.05 :font-size 10
    :value value :min lo :max hi :decimals 3 :unit " s"
    :on-change on-change))

(def clip-section (r)
  (let ((start (region-window-start r))
        (end (region-window-end r))
        (duration r.duration))
    (v-stack :key "audio-clip-clip" :width :fill :gap 0.15
      (section-label "audio-clip-clip-label" "Clip")
      (h-stack :width :fill :gap 0.4
        (v-stack :gap 0.1
          (section-label "audio-clip-start-label" (if r.loop "Position" "Start"))
          (seconds-field "audio-clip-start" start 0 (max 0 (- end 0.001))
            (lambda (v) (set-window! r v end true))))
        (v-stack :gap 0.1
          (section-label "audio-clip-end-label" (if r.loop "Loop end" "End"))
          (seconds-field "audio-clip-end" end (+ start 0.001) duration
            (lambda (v) (set-window! r start v true)))))
      ;; Looping: extending the region on the timeline repeats the loop.
      (button "Loop" :key "audio-clip-loop" :width :fill :height 1.05 :font-size 10
        :active r.loop
        :active-background-color '(rgba 0.95 0.62 0.25 1)
        :active-color :black
        :on-click (lambda (e) (host-command "audio-region-set" (dict :id r.cid :loop (not r.loop))))))))

(def audio-section (r)
  (v-stack :key "audio-clip-audio" :width :fill :gap 0.15
    (section-label "audio-clip-audio-label" "Audio")
    (h-stack :width :fill :gap 0.4 :align :center
      (section-label "audio-clip-gain-label" "Gain")
      (hslider :key "audio-clip-gain" :width 6 :min -36 :max 12
        :value r.gain
        :on-change (lambda (v)
          (host-command "audio-region-set" (dict :id r.cid :gain v :live true))))
      (label (gain-label r.gain) :key "audio-clip-gain-value" :font-size 10 :bg :transparent))
    (h-stack :width :fill :gap 0.4 :align :center
      (section-label "audio-clip-pitch-label" "Pitch")
      (knob :key "audio-clip-pitch" :size 3.5 :min -24 :max 24 :value r.pitch
        :on-change (lambda (v)
          (host-command "audio-region-set" (dict :id r.cid :pitch (round v) :live true))))
      (label (pitch-label r.pitch) :key "audio-clip-pitch-value" :font-size 10 :bg :transparent))
    ;; Warp and reverse arrive with the sampler-shaped device
    ;; (docs/audio-tracks-spec.md §6.3, §8.3).
    (h-stack :width :fill :gap 0.4
      (button "Warp" :key "audio-clip-warp" :width 6.6 :height 1.05 :font-size 10 :disabled true)
      (button "Reverse" :key "audio-clip-reverse" :width 6.6 :height 1.05 :font-size 10 :disabled true))))

(def clip-panel (r)
  (box :key "audio-clip-panel" :width panel-width :height :fill :padding 0.4
    :background-color :mixer-strip-bg
    (v-stack :width :fill :gap 0.4
      (label (truncate-text (region-label r) 24) :key "audio-clip-name" :font-size 11 :bg :transparent)
      (clip-section r)
      (audio-section r))))

(def waveform-view (r)
  (subtree :key (str "audio-clip-waveform-" r.sample)
    (box :flex 1 :width 0 :height :fill :padding 0.2
      (waveform
        :key "audio-clip-waveform"
        :width :fill
        :height :fill
        :header-height 0.9
        :ruler-font-size 9
   ;     :ruler-color :sampler-grid
    ;    :ruler-bg :sampler-control-bg
        :grid-major-color :sampler-grid
        :grid-minor-color :sampler-grid
;        :bg :sampler-control-bg
        :focusable true
        :marker-selection true
        ;; Ableton's clip view: a start/end bar under the ruler with thin
        ;; lines through the waveform, not the sampler's flags.
        :marker-style :bar
        :marker-color '(rgba 0.62 0.82 0.93 1)
        :active-marker-color :widget-knob-filled
        :waveform-color :fg
        :inactive-waveform-color '(rgba 0.32 0.32 0.32 1)
        :buffer (dict :registry-key r.sample)
        :view-start audio-clip.view-start
        :view-duration (if (= audio-clip.view-duration 0) r.duration audio-clip.view-duration)
        :cursor-time audio-clip.cursor
        ;; The playhead follows the song: the widget maps the bound song
        ;; position through the region (and its loop) onto source seconds,
        ;; so this view never reruns per frame. It shows only while the song
        ;; is inside THIS region (a region being viewed while another plays
        ;; shows none) and only while it plays.
        :playhead-beat (if (= song.mode "stopped") nil #'song.position)
        ;; loop-start/-end are 0 when the region does not loop, which the
        ;; widget reads as no loop.
        :playhead-map (dict :start r.start :end r.end :offset r.offset :span r.span
                        :loop-start r.loop-start :loop-end r.loop-end)
        :selection-start (window-start r)
        :selection-end (window-end r)
        :range-start (when audio-clip.range (get audio-clip.range :start))
        :range-end (when audio-clip.range (get audio-clip.range :end))
        :time-ruler (dict :mode :seconds)
        :on-focus-key focus-key
        :on-action |event| (waveform-action r event)))))

;; The editor for track t's bound region.
;; ── Keys ───────────────────────────────────────────────────────────────────
;; The arrangement region's copy / paste / duplicate, shared with the
;; arrangement lanes. Each returns whether it handled the key.

(def audio-track? (t)
  (and (not (= t nil)) (= t.instrument-type "audio")))

(def song-region-copy ()
  (if (= song.region nil) false (do (seq-song-region-copy) true)))

(def song-region-paste ()
  (seq-song-region-paste)
  true)

(def song-region-duplicate ()
  (if (= song.region nil) false (do (seq-song-region-duplicate) true)))

;; The region keys as a focused widget's :on-focus-key receives them: Edit >
;; Copy/Paste reach a focused widget first, and one that refuses them hands
;; them on. Cmd on macOS, Ctrl elsewhere.
(def song-region-key (key)
  (match key
    "s-c" (song-region-copy)
    "C-c" (song-region-copy)
    "s-v" (song-region-paste)
    "C-v" (song-region-paste)
    "s-d" (song-region-duplicate)
    "C-d" (song-region-duplicate)
    _ false))

;; The editor shares the *piano-roll* buffer. Its keymap adds the region keys
;; while an audio track is being edited, so a range dragged across the
;; waveform (which selects the same timeline stretch) copies, pastes and
;; duplicates with Cmd+C / Cmd+V / Cmd+D. Otherwise each returns false and
;; the key falls through to the piano roll as before.
(def editing-audio? () (audio-track? piano-roll.track))
(def region-copy-key () (if (editing-audio?) (song-region-copy) false))
(def region-paste-key () (if (editing-audio?) (song-region-paste) false))
(def region-duplicate-key () (if (editing-audio?) (song-region-duplicate) false))
(def focus-key (key text) (if (editing-audio?) (song-region-key key) false))

(define-mode "audio-clip-mode" :read-only true :live-keys true
  :inherit "eseq.sequencer-keys/sequencer-keys")
(mode-bind-key "audio-clip-mode" "s-c" "region-copy-key")
(mode-bind-key "audio-clip-mode" "C-S-c" "region-copy-key")
(mode-bind-key "audio-clip-mode" "s-v" "region-paste-key")
(mode-bind-key "audio-clip-mode" "C-S-v" "region-paste-key")
(mode-bind-key "audio-clip-mode" "s-d" "region-duplicate-key")
(mode-bind-key "audio-clip-mode" "C-d" "region-duplicate-key")

(def editor (t)
  (let ((r (bound-region t)))
    (if (= r nil)
      (box :key "audio-clip-empty" :width :fill :height :fill :h-align :center :v-align :center
        (label "Double-click an audio region to edit it"
          :key "audio-clip-empty-label" :font-size 11 :color :dim :bg :transparent))
      (h-stack :key "audio-clip-editor" :width :fill :height :fill :gap 0
        (subtree :key "audio-clip-panel-tree" (clip-panel r))
        (waveform-view r)))))
