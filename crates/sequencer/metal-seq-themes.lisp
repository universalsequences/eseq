;; metal-seq-themes — command-facing sequencer theme registry

(defstate seq-current-theme "mac-osx-dark")

(def seq-theme-registry ()
  (list
    (dict :name "mac-osx-dark" :command "seq-theme-mac-osx-dark" :file "mac-osx-dark.lisp")
    (dict :name "mac-osx-graphite" :command "seq-theme-mac-osx-graphite" :file "mac-osx-graphite.lisp")
    (dict :name "mac-osx-haze" :command "seq-theme-mac-osx-haze" :file "mac-osx-haze.lisp")
    (dict :name "mac-osx-midnight" :command "seq-theme-mac-osx-midnight" :file "mac-osx-midnight.lisp")
    (dict :name "black-ir-theme" :command "seq-theme-black-ir" :file "black-ir-theme.lisp")
    (dict :name "mac-osx-ember" :command "seq-theme-mac-osx-ember" :file "mac-osx-ember.lisp")
    (dict :name "mac-osx-violet" :command "seq-theme-mac-osx-violet" :file "mac-osx-violet.lisp")))

(def seq-apply-theme-file (name file)
  (do
    (load file)
    (set! seq-current-theme name)
    (status (fmt "Theme applied: {}" name))))

(def seq-theme-mac-osx-dark ()
  (seq-apply-theme-file "mac-osx-dark" "mac-osx-dark.lisp"))

(def seq-theme-mac-osx-graphite ()
  (seq-apply-theme-file "mac-osx-graphite" "mac-osx-graphite.lisp"))

(def seq-theme-mac-osx-haze ()
  (seq-apply-theme-file "mac-osx-haze" "mac-osx-haze.lisp"))

(def seq-theme-mac-osx-midnight ()
  (seq-apply-theme-file "mac-osx-midnight" "mac-osx-midnight.lisp"))

(def seq-theme-black-ir ()
  (seq-apply-theme-file "black-ir-theme" "black-ir-theme.lisp"))

(def seq-theme-mac-osx-ember ()
  (seq-apply-theme-file "mac-osx-ember" "mac-osx-ember.lisp"))

(def seq-theme-mac-osx-violet ()
  (seq-apply-theme-file "mac-osx-violet" "mac-osx-violet.lisp"))
