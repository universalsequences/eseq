(load "../eseqlisp/themes.lisp")
(load "mac-osx-theme.lisp")
(mac-osx-theme)

(load "music-player-library.lisp")
(load "music-player-album.lisp")
(load "music-player-now-playing.lisp")

(set-layout '(:cols
  0.34 (:buf "*library*" :hide-status true :borderless true :min-width 28)
  0.28 (:buf "*album*" :hide-status true :borderless true :min-width 40)
  0.38 (:buf "*now-playing*" :hide-status true :borderless true :min-width 32)))
