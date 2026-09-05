;; ui/midi.lisp — hardware MIDI mapping core (bead eseq-egs6 follow-up).
;;
;; Pure data + dispatch, no UI. Every message a hardware port delivers
;; reaches `dispatch` on the UI thread as a map:
;;
;;   {:kind :cc         :channel 0 :cc 14 :raw 100 :value 0.787 :port 0}
;;   {:kind :note-on    :channel 0 :note 60 :velocity 0.8 :port 0}
;;   {:kind :note-off   :channel 0 :note 60 :velocity 0.0 :port 0}
;;   {:kind :pitch-bend :channel 0 :value -0.5 :port 0}
;;   {:kind :aftertouch :channel 0 :value 0.3 :port 0}
;;
;; `:value` is always normalised (0..1, or -1..1 for pitch bend) so targets
;; never see raw 7-bit numbers unless they ask for `:raw`.
;;
;; Mappings are a list of plain dicts so a future editor UI can render and
;; edit the same table the user's init.lisp builds:
;;
;;   {:key "cc:14" :source {...} :target {...} :mode :absolute}
;;
;; Sources and targets are constructed with the small helpers below and
;; resolved at EVENT time — `(rack-macro 0)` means "macro 0 of whichever
;; Instrument Rack is armed right now", so one mapping follows the player
;; from synth to synth. A target may also be any callable
;; `(lambda (value msg) …)` for one-off scripting.
;;
;; Typical ~/.eseq.d/init.lisp:
;;
;;   (import eseq.midi :refer (midi-map cc rack-macro))
;;   (midi-map (cc 14) (rack-macro 0))
;;   (midi-map (cc 15) (rack-macro 1))
;;   (midi-map (on-channel 9 (note 36)) (lambda (velocity msg) …))
;;
;; `dispatch` returns true when a mapping consumed the message. A consumed
;; note does NOT reach the live keyboard (so a pad row can be turned into
;; anything); everything else falls through to the built-in note path.
(module eseq.midi)

(export mappings
        midi-map
        midi-map*
        midi-unmap
        midi-clear-mappings
        midi-mappings
        cc
        note
        pitch-bend
        aftertouch
        on-channel
        rack-macro
        rack-macro-of
        armed-rack-track
        source-key
        dispatch
        last-message)

;; The mapping table. A defstate so a mapping editor can bind to it.
(defstate mappings (list))

;; Most recent message seen, for "MIDI learn" style UIs and debugging.
(defstate last-message nil)

;; Raw listeners: `(add-hook "midi-message-hook" "my-key" (lambda (msg) …))`.
;; Flat hook keyspace — do not rename.
(defhook "midi-message-hook")

;; 0 is falsy in eseqlisp, so channel/track/value presence is always tested
;; against nil explicitly rather than by truthiness.
(def present? (value) (not (= value nil)))

(def or-default (value fallback)
  (if (= value nil) fallback value))

;; ── Sources ────────────────────────────────────────────────────────────────

;; Sources match on any channel unless wrapped with `on-channel`.
;; Functions here are fixed-arity (no &rest outside macros), so options are
;; layered with small wrappers rather than keyword arguments.
(def cc (controller)
  (dict :kind :cc :cc controller))

(def note (number)
  (dict :kind :note :note number))

(def pitch-bend ()
  (dict :kind :pitch-bend))

(def aftertouch ()
  (dict :kind :aftertouch))

;; (on-channel 1 (cc 14)) → the same source restricted to channel 1 (0-based).
(def on-channel (channel source)
  (merge source :channel channel))

;; Stable identity of a source, used as the mapping key so re-evaluating
;; init.lisp replaces a mapping instead of stacking a duplicate.
(def source-key (source)
  (let ((kind (get source :kind))
        (channel (get source :channel)))
   (let ((suffix (if (present? channel) (str "@" channel) "")))
    (if (= kind :cc)
      (str "cc:" (get source :cc) suffix)
      (if (= kind :note)
        (str "note:" (get source :note) suffix)
        (str (get source :kind) suffix))))))

(def source-matches? (source msg)
  (let ((kind (get source :kind))
        (channel (get source :channel))
        (msg-kind (get msg :kind)))
    (if (and (present? channel) (not (= channel (get msg :channel))))
      false
      (if (= kind :cc)
        (and (= msg-kind :cc) (= (get source :cc) (get msg :cc)))
        (if (= kind :note)
          (and (or (= msg-kind :note-on) (= msg-kind :note-off))
               (= (get source :note) (get msg :note)))
          (= kind msg-kind))))))

;; ── Targets ────────────────────────────────────────────────────────────────

;; (rack-macro 0) → macro 0 of the armed Instrument Rack.
(def rack-macro (index)
  (dict :kind :rack-macro :macro index))

;; (rack-macro-of 3 0) → macro 0 of track 3, armed or not.
(def rack-macro-of (track index)
  (dict :kind :rack-macro :macro index :track track))

;; First armed track whose instrument is an Instrument Rack, else nil.
;; Exported so scripts can `override` the policy (e.g. prefer the selected
;; track when nothing is armed).
(def armed-rack-track ()
  (first (filter |track| (seq-track-is-rack? track) (seq-armed-tracks))))

;; ── Mapping table ──────────────────────────────────────────────────────────

;; (midi-map source target) — absolute mapping.
;; (midi-map* source target (dict :mode :relative :step 0.02)) — with options.
;; Relative mode treats the CC as an endless encoder (two's-complement
;; deltas, 1..63 up / 65..127 down) and nudges the target from its current
;; value; `:step` scales one detent (default 1/127).
(def midi-map (source target)
  (midi-map* source target (dict)))

(def midi-map* (source target opts)
  (let ((key (source-key source)))
   (let ((mapping (dict :key key
                       :source source
                       :target target
                       :mode (or-default (get opts :mode) :absolute)
                       :step (or-default (get opts :step) (/ 1.0 127.0)))))
    (set! mappings
      (append (filter |m| (not (= (get m :key) key)) mappings)
              (list mapping)))
    mapping)))

(def midi-unmap (source)
  (let ((key (source-key source)))
    (set! mappings (filter |m| (not (= (get m :key) key)) mappings))
    true))

(def midi-clear-mappings ()
  (set! mappings (list))
  true)

(def midi-mappings () mappings)

;; ── Dispatch ───────────────────────────────────────────────────────────────

(def relative-delta (raw)
  (if (< raw 64) raw (- raw 128)))

(def apply-rack-macro (target mapping msg)
  (let ((track (or-default (get target :track) (armed-rack-track)))
        (index (get target :macro)))
    (if (and (present? track) (seq-track-is-rack? track))
      (let ((value (if (= (get mapping :mode) :relative)
                     (let ((current (seq-rack-macro-value track index)))
                       (clamp (+ (or-default current 0.0)
                                 (* (relative-delta (get msg :raw)) (get mapping :step)))
                              0.0 1.0))
                     (get msg :value))))
        ;; Same command pair as the on-screen macro knob (instrument-panel
        ;; `rack-macro-set`): with steps selected the turn writes a p-lock
        ;; onto them, otherwise it sets the value — and, while play+record
        ;; is on, the host's print latch prints it onto passing steps.
        (host-command (if (seq-has-selection?) "set-rack-macro-plock" "set-rack-macro-value")
          (dict :track track :id index :value value))
        true)
      false)))

(def apply-target (target mapping msg)
  (let ((kind (get target :kind)))
    (if (= kind :rack-macro)
      (apply-rack-macro target mapping msg)
      (if (present? kind)
        false
        ;; Not a target dict: treat as a callable (lambda (value msg) …).
        (do (target (get msg :value) msg) true)))))

;; Entry point the host calls once per message. Returns true when consumed.
(def dispatch (msg)
  (set! last-message msg)
  (run-hook "midi-message-hook" msg)
  (reduce |consumed mapping|
    (if (source-matches? (get mapping :source) msg)
      (or (apply-target (get mapping :target) mapping msg) consumed)
      consumed)
    false
    mappings))
