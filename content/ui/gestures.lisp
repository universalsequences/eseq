;; ui/gestures.lisp — pointer gestures and selection, independent of looks.
;;
;;   (apply box … (drag-gesture :begin f :move g :end h) children)
;;   (select-click e x xs selected anchor)

(module eseq.gestures)

(export drag-gesture select-click)

;; The press-drag-release in progress: what `begin` returned, and where the
;; press was. One at a time (there is one pointer).
(def current nil)

;; Box props for a press-drag-release gesture that keeps the pointer past the
;; box's edges. (begin e) on the press returns the gesture's state, or nil to
;; let the press go (no gesture); (move e state rows cols) on each drag gets
;; how far the pointer is from the press, rows down and cols right;
;; (end e state) on the release.
(def drag-gesture (&key begin move end)
  (list :capture-pointer true
        :on-mouse-down (lambda (e)
                         (let ((s (begin e)))
                           (set! current (if (= s nil) nil (dict :state s :row e.row :col e.col)))))
        :on-drag (lambda (e)
                   (when (and current move)
                     (move e (get current :state) (- e.row (get current :row))
                           (- e.col (get current :col)))))
        :on-mouse-up (lambda (e)
                       (when current
                         (do (when end (end e (get current :state)))
                             (set! current nil))))))

;; A click on x among xs (in their shown order), as lists select: Shift
;; selects the range from `anchor` to x, an additive click (Cmd) toggles x,
;; a plain click selects x alone. Returns (dict :selected xs' :anchor a').
(def select-click (e x xs selected anchor)
  (let ((pos (lambda (y) (reduce |found i| (if (= (nth xs i) y) i found) -1 (range 0 (len xs))))))
    (if (and e.shift anchor (>= (pos anchor) 0))
      (let ((a (pos anchor)) (b (pos x)))
        (dict :selected (map (lambda (i) (nth xs i)) (range (min a b) (+ 1 (max a b))))
              :anchor anchor))
      (if e.additive-selection
        (dict :selected (if (> (len (filter (lambda (y) (= y x)) selected)) 0)
                          (filter (lambda (y) (not (= y x))) selected)
                          (append selected (list x)))
              :anchor anchor)
        (dict :selected (list x) :anchor x)))))
