;; eseq.gen.panel — building blocks for a generated instrument's ui.lisp
;; (docs/program-generators-spec.md §5.3), in the style of
;; .local/instruments/modalys-pluck: a panel per section with its first
;; params as knobs, and a screen showing the selected section's page of
;; number fields. eseq.modal adds its patch drawing to the screen.
;;
;; A section is (dict :title :tab :params): its panel heading, its screen
;; tab's label, and its param specs (eseq.gen's pspec). A section's number
;; (k below) is its position in the instrument's list of sections.

(module eseq.gen.panel)

(import eseq.gen :refer (take host-name))

(export ui-helpers control-form num-form panel-form page-form pages-from screen-form sections-ui-forms)

;; The definitions every generated panel starts with, as forms. They are
;; named mx-* to keep them apart from the user's own code when they edit the
;; result. In them, section is a section number, name a param's host name
;; (see host-name), title a label, and width a size in panel units.
;;   mx-c        the panel's one accent colour
;;   mx-section  the selected section's number
;;   mx-p        the host's param record for name
;;   mx-select   a callback selecting section
;;   mx-knob     a knob (decimals shown, :linear / :log taper)
;;   mx-label    a panel's heading strip
;;   mx-panel    a section's panel holding body; clicking it selects section
;;   mx-option   a dropdown for a menu param (options: its option strings)
;;   mx-num      a number field for the screen's param page
;;   mx-chip     an object in the screen's patch drawing
;;   mx-link     a connection or mic in the drawing
;;   mx-tab      a tab under the screen selecting section
(def ui-helpers ()
  '((def mx-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
    (def mx-section () eseq.vanilla/custom-ui-selected-section)
    (def mx-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
    (def mx-select (section) (eseq.effects.custom-ui-sections/ui-section-select-callback section))
    (def mx-knob (section name title decimals taper)
      (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 5.6 3.45 2.15
        (mx-c) decimals taper :widget-knob-track 8.5 8 :center))
    (def mx-label (title width)
      (box :width width :height 0.72 :background-color (mx-c)
        (label title :width width :height 0.72 :h-align :center :v-align :center
          :font-size 8.2 :color :black :bg :transparent)))
    (def mx-panel (section title width body)
      (box :width width :height 4.8 :padding 0.18
        :background-color (if (= (mx-section) section) :instrument-panel-bg :instrument-group-bg)
        :debug-name (str "modal-panel-" section)
        :on-click (mx-select section)
        (v-stack :gap 0.14 (mx-label title (min width 9)) body)))
    (def mx-option (section name title options)
      (let ((p (mx-p name)) (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-opt-mod-" name)
          (box :width 6.6 :height 3.45 :v-align :center
            (v-stack :width 6.6 :height 1.4 :gap 0.12
              (label title :width 6.6 :height 0.5 :h-align :center :v-align :center
                :font-size 8.5 :bg :transparent)
              (dropdown :debug-name (str "modal-" name) :width 6.6 :height 0.7 :font-size 7.2
                :options options
                :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :text-color (mx-c) :chevron-color (mx-c) :bg-color :instrument-control-bg :border-color :transparent
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (lambda (v)
                  (do
                    (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                      (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
    (def mx-num (section name title width decimals)
      (let ((p (mx-p name))
            (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-mod-" name)
          (subtree :key (str "modal-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
              (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
            (v-stack :width width :height 1.15 :gap 0.08
              (label title :height 0.5 :v-align :center :font-size 9.5 :color ink :bg :transparent)
              (number-picker :width width :height 0.55 :noui true :font-size 10 :decimals decimals
                :unit (eseq.effects.custom-ui-runtime/custom-ui-param-control-unit p)
                :step (pow 10 (- 0 decimals))
                :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
                :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
                :text-align :left
                :text-color ink :edit-color ink :cursor-color ink
                :plock-style :underline
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))
    ;; the screen's patch drawing: objects as chips, connections as links
    (def mx-chip (section title)
      (button title :width (+ 1.4 (* 0.62 (len title))) :height 0.8 :padding 0 :font-size 8.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :black
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))
    (def mx-link (section title)
      (button title :width (+ 1 (* 0.55 (len title))) :height 0.8 :padding 0 :font-size 7.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black :transparent)
        :on-click (mx-select section)))
    (def mx-tab (section title width)
      (button title :width width :height 0.8 :padding 0 :font-size 8
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))))

;; A panel control form for a param: a dropdown for a menu, else a knob.
;;   k  the section number
;;   p  a param spec
(def control-form (k p)
  (if (get p :options)
    `(mx-option ,k ,(host-name p) ,(get p :title) (quote ,(get p :options)))
    `(mx-knob ,k ,(host-name p) ,(get p :title) ,(get p :dec) ,(get p :taper))))

;; A number field form for a param on the screen's page.
;;   k      the section number
;;   p      a param spec
;;   width  the field's width
(def num-form (k p width)
  `(mx-num ,k ,(host-name p) ,(get p :title) ,width ,(get p :dec)))

;; A section's panel form, holding controls for its first params.
;;   k      the section number
;;   sec    the section
;;   knobs  how many params get a control
;;   width  the panel's width
(def panel-form (k sec knobs width)
  `(mx-panel ,k ,(get sec :title) ,width
     (h-stack :gap 0.15 ,@(map (lambda (p) (control-form k p)) (take (get sec :params) knobs)))))

;; The screen's page for one section: a number field for every param, six to
;; a row. Menu params are left out; their dropdown is on the panel.
;;   k    the section number
;;   sec  the section
(def page-form (k sec)
  `(v-stack :gap 0.12
     ,@(map (lambda (row) `(h-stack :gap 0.35 ,@(map (lambda (p) (num-form k p 5.3)) row)))
            (chunks (filter (lambda (p) (not (get p :options))) (get sec :params)) 6))))

;; A form showing the selected section's page: nested ifs over the sections
;; from k on, the last one shown when none before it is selected.
;;   secs  the sections
;;   k     the first section number to test
(def pages-from (secs k)
  (if (= k (- (len secs) 1))
    (page-form k (nth secs k))
    `(if (= (mx-section) ,k) ,(page-form k (nth secs k)) ,(pages-from secs (+ k 1)))))

;; The screen form: a title naming the instrument and the selected section,
;; an optional drawing, the selected section's page, and a tab per section.
;;   name     the instrument's name, for the title
;;   secs     the sections
;;   drawing  a form drawn above the page (eseq.modal: the patch), or nil
;;            to give the page its room
(def screen-form (name secs &key (drawing nil))
  (let ((tab-w (/ (- 35.2 (* 0.18 (- (len secs) 1))) (len secs))))
    `(box :debug-name "modal-display" :part "instrument/screen" :width 35.8 :height 9.8 :padding 0.3
          :background-color (mx-c)
       (v-stack :gap 0.15
         (label (str ,(str name " / ") (nth (quote ,(map (lambda (s) (get s :title)) secs)) (mx-section)))
           :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (mx-c) :bg :black)
         ,@(if drawing
             (list `(box :width 35.2 :height 4.1 ,drawing) `(box :width 35.2 :height 2.6 ,(pages-from secs 0)))
             (list `(box :width 35.2 :height 6.85 ,(pages-from secs 0))))
         (h-stack :gap 0.18
           ,@(map (lambda (k) `(mx-tab ,k ,(get (nth secs k) :tab) ,tab-w)) (range (len secs))))))))

;; Every form of a sectioned ui.lisp: the mx-* helpers, then a defsynth-ui
;; with the screen and, right of it, each section's panel, two to a column.
;; What a generator with no drawing of its own can return as its :ui.
;;   name   the instrument's name
;;   secs   the sections
;;   knobs  how many of a section's params its panel shows
(def sections-ui-forms (name secs &key (knobs 3))
  (append (ui-helpers)
          (list `(defsynth-ui
                   (h-stack :height 9.8 :gap 0.3 :align :start
                     ,(screen-form name secs)
                     ,@(map (lambda (pair) `(v-stack :gap 0.2 ,@pair))
                            (chunks (map (lambda (k) (panel-form k (nth secs k) knobs 18.7)) (range (len secs))) 2)))))))
