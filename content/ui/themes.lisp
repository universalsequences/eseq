;; Command-facing sequencer theme registry.

(defstate seq-current-theme "mac-osx-dark")

(def seq-theme-registry ()
  (list
    (dict :name "mac-osx-dark" :label "macOS Dark" :command "seq-theme-mac-osx-dark" :file "@/ui/themes/mac-osx-dark.lisp")
    (dict :name "mac-osx-light-theme" :label "macOS Light" :command "seq-theme-mac-osx-light" :file "@/ui/themes/mac-osx-light-theme.lisp")
    (dict :name "ableton-mid" :label "Ableton Mid" :command "seq-theme-ableton-mid" :file "@/ui/themes/ableton-mid.lisp")
    (dict :name "mac-osx-graphite" :label "macOS Graphite" :command "seq-theme-mac-osx-graphite" :file "@/ui/themes/mac-osx-graphite.lisp")
    (dict :name "mac-osx-haze" :label "macOS Haze" :command "seq-theme-mac-osx-haze" :file "@/ui/themes/mac-osx-haze.lisp")
    (dict :name "mac-osx-rune" :label "macOS Rune" :command "seq-theme-mac-osx-rune" :file "@/ui/themes/mac-osx-rune.lisp")
    (dict :name "mac-osx-midnight" :label "macOS Midnight" :command "seq-theme-mac-osx-midnight" :file "@/ui/themes/mac-osx-midnight.lisp")
    (dict :name "mac-osx-midnight-50" :label "macOS Midnight 50" :command "seq-theme-mac-osx-midnight-50" :file "@/ui/themes/mac-osx-midnight-50.lisp")
    (dict :name "black-ir-theme" :label "Black IR" :command "seq-theme-black-ir" :file "@/ui/themes/black-ir-theme.lisp")
    (dict :name "mac-osx-ember" :label "macOS Ember" :command "seq-theme-mac-osx-ember" :file "@/ui/themes/mac-osx-ember.lisp")
    (dict :name "mac-osx-violet" :label "macOS Violet" :command "seq-theme-mac-osx-violet" :file "@/ui/themes/mac-osx-violet.lisp")
    (dict :name "tahoe-terminal" :label "Tahoe Terminal" :command "seq-theme-tahoe-terminal" :file "@/ui/themes/tahoe-terminal.lisp")
    (dict :name "phosphor" :label "Phosphor" :command "seq-theme-phosphor" :file "@/ui/themes/phosphor.lisp")
    (dict :name "phosphor-blue" :label "Phosphor Blue" :command "seq-theme-phosphor-blue" :file "@/ui/themes/phosphor-blue.lisp")
    (dict :name "aura" :label "Aura" :command "seq-theme-aura" :file "@/ui/themes/aura.lisp")))

;; The palette's "Change Theme" choices: every registered theme by label.
(def seq-theme-choices ()
  (map (lambda (theme) (dict :label (get theme :label) :value (get theme :name)))
       (seq-theme-registry)))

(def seq-current-theme-name () seq-current-theme)

(def change-theme (theme)
  :doc "Switch the UI theme.
Lists every theme in the registry, with the current one checked."
  :command "Change Theme"
  :prompt "Theme"
  :choices seq-theme-choices
  :current seq-current-theme-name
  (let ((entry (find-by-key (seq-theme-registry) :name theme)))
    (if (= entry nil)
      (status (str "Unknown theme: " theme))
      (seq-apply-theme-file theme (get entry :file)))))

(def seq-apply-theme-file (name file)
  (do
    (load file)
    (set! seq-current-theme name)
    (status (fmt "Theme applied: {}" name))))

(def seq-theme-mac-osx-dark ()
  (seq-apply-theme-file "mac-osx-dark" "@/ui/themes/mac-osx-dark.lisp"))

(def seq-theme-mac-osx-light ()
  (seq-apply-theme-file "mac-osx-light-theme" "@/ui/themes/mac-osx-light-theme.lisp"))

(def seq-theme-ableton-mid ()
  (seq-apply-theme-file "ableton-mid" "@/ui/themes/ableton-mid.lisp"))

(def seq-theme-mac-osx-graphite ()
  (seq-apply-theme-file "mac-osx-graphite" "@/ui/themes/mac-osx-graphite.lisp"))

(def seq-theme-mac-osx-haze ()
  (seq-apply-theme-file "mac-osx-haze" "@/ui/themes/mac-osx-haze.lisp"))

(def seq-theme-mac-osx-rune ()
  (seq-apply-theme-file "mac-osx-rune" "@/ui/themes/mac-osx-rune.lisp"))

(def seq-theme-mac-osx-midnight ()
  (seq-apply-theme-file "mac-osx-midnight" "@/ui/themes/mac-osx-midnight.lisp"))

(def seq-theme-mac-osx-midnight-50 ()
  (seq-apply-theme-file "mac-osx-midnight-50" "@/ui/themes/mac-osx-midnight-50.lisp"))

(def seq-theme-black-ir ()
  (seq-apply-theme-file "black-ir-theme" "@/ui/themes/black-ir-theme.lisp"))

(def seq-theme-mac-osx-ember ()
  (seq-apply-theme-file "mac-osx-ember" "@/ui/themes/mac-osx-ember.lisp"))

(def seq-theme-mac-osx-violet ()
  (seq-apply-theme-file "mac-osx-violet" "@/ui/themes/mac-osx-violet.lisp"))

(def seq-theme-tahoe-terminal ()
  (seq-apply-theme-file "tahoe-terminal" "@/ui/themes/tahoe-terminal.lisp"))

(def seq-theme-phosphor ()
  (seq-apply-theme-file "phosphor" "@/ui/themes/phosphor.lisp"))

(def seq-theme-phosphor-blue ()
  (seq-apply-theme-file "phosphor-blue" "@/ui/themes/phosphor-blue.lisp"))

(def seq-theme-aura ()
  (seq-apply-theme-file "aura" "@/ui/themes/aura.lisp"))
