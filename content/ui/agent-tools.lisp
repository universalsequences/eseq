;; ui/agent-tools.lisp — the agent tool registry (bead eseq-maxc).
;;
;; A tool is a name, a doc the model reads to decide when to call it, its
;; parameters, and a function of one argument: a map of what the model
;; passed. `metal_seq mcp` lists these beside its host tools (eval, outline,
;; define_form, capture, …) and calls them through `call-tool`; the in-app
;; agent will read the same registry. Any module can add one:
;;
;;   (import eseq.agent-tools :refer (define-agent-tool))
;;   (define-agent-tool "track_names"
;;     :doc "The project's track names, in order."
;;     :run (lambda (args) …))
;;
;; Parameters are `(name :type "doc")`, with :type :string, :number,
;; :integer or :bool, plus `:optional true` for one the model may leave out:
;;
;;   :params '((bars :integer "How many bars" :optional true))
;;
;; `run` reads them as `args.bars`. It returns a string, shown as is, or any
;; value, which is printed. Defining a name again replaces the tool.

(module eseq.agent-tools)

(import eseq.kinds :refer (tracks))

(export define-agent-tool tool-specs has-tool? call-tool)

(defstate registry (list))

(def define-agent-tool (name &key doc params run)
  (set! registry
    (append (filter |tool| (not (= (get tool :name) name)) registry)
            (list (dict :name name :doc doc :params params :run run)))))

;; What the host lists: every tool without its function.
(def tool-specs ()
  (map |tool| (dict :name (get tool :name) :doc (get tool :doc) :params (get tool :params))
       registry))

(def has-tool? (name)
  (not (empty? (filter |tool| (= (get tool :name) name) registry))))

(def call-tool (name args)
  (let ((tool (first (filter |tool| (= (get tool :name) name) registry))))
    (if (= tool nil)
      (str "no agent tool " name)
      (apply (get tool :run) (list args)))))

(define-agent-tool "track_names"
  :doc "The project's tracks, one per line: index and name. Sequencer kinds route their notes to tracks by index."
  :run (lambda (args)
         (let ((names (map |track| track.name (tracks))))
           (reduce |acc i| (str acc i "  " (nth names i) "\n") "" (range (len names))))))
