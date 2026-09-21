;; Use with --project and --all-panels to inspect a restored script tab beside Seq.
(capture-project)

(def capture-after-sync ()
  (let ((buffer (eseq.seq-step-tabs/seq-step-tab-buffer
                  (nth (eseq.seq-step-tabs/seq-main-step-tabs) 1))))
    ;; Capture starts with transport focused; address the main tile explicitly.
    (set! eseq.seq-step-tabs/step-panel-buffer buffer)
    (set! eseq.seq-step-tabs/remembered-step-panel-buffer buffer)
    (set-window-buffer-for "*sequencer*" buffer)))
