;; Save Region: right-click a sampler's waveform to keep its start/end range
;; as a library sample of its own, named and tagged here. The host
;; (src/ui/host_commands/sample_region.rs) remembers which sample and range
;; the menu was opened on; Save cuts it into the library, recording the
;; parent so the browser can list regions and say where each came from.
;;
;; Packs (curated lists kept apart from tags) are filled from here too:
;; Save Region can drop the region into one, and the browser's New Pack…
;; opens `new-pack-panel` on a sample.
;;
;; Mounted by both step-panel buffers, like Resample; Rust activates the
;; mount's tile before calling `open` or `open-new-pack`.
(module eseq.sample-region)
(import eseq.sample-import)
(export region-view new-pack-view panel new-pack-panel open close show-error
        open-new-pack close-new-pack pack-suggestions)

;; The modal: the parent's title and the range's text (display only; the
;; host holds the range itself), the name, tags and pack ("" for none), and
;; the last error.
(def-kind region-view
  :key ()
  :state ((open false)
          (parent "")
          (span "")
          (name "")
          (tags '())
          (tag-draft "")
          (pack "")
          (error "")))

;; New Pack: the sample (a browser path) it starts with, and its name.
(def-kind new-pack-view
  :key ()
  :state ((open false)
          (path "")
          (name "")))

;; Tags and pack default to the last region's, so a run of cuts from one
;; break keeps its labels and lands in the same set.
(def open (default-name default-tags parent span default-pack)
  (set! region-view.parent parent)
  (set! region-view.span span)
  (set! region-view.name default-name)
  (set! region-view.tags default-tags)
  (set! region-view.tag-draft "")
  (set! region-view.pack default-pack)
  (set! region-view.error "")
  (set! region-view.open true))

(def close ()
  (set! region-view.open false)
  (set! region-view.tag-draft ""))

(def show-error (message)
  (set! region-view.error message))
(def cancel () (host-command "sample-region-close" (dict)))

(def has-tag? (tag)
  (> (len (filter |existing| (= (string-downcase existing) (string-downcase tag))
             region-view.tags))
     0))
(def add-tag (tag)
  (if (or (= tag "") (has-tag? tag)) (set! region-view.tag-draft "")
    (do (set! region-view.tags (append region-view.tags (list tag)))
        (set! region-view.tag-draft ""))))
(def remove-tag (tag)
  (set! region-view.tags (filter |existing| (not (= existing tag)) region-view.tags)))

;; A tag typed but not yet entered still counts.
(def commit ()
  (add-tag region-view.tag-draft)
  (host-command "sample-region-commit"
    (dict :name region-view.name :tags region-view.tags :pack region-view.pack)))

(def open-new-pack (path)
  (set! new-pack-view.path path)
  (set! new-pack-view.name "")
  (set! new-pack-view.open true))
(def close-new-pack ()
  (set! new-pack-view.open false))
(def create-pack ()
  (host-command "sample-pack-add" (dict :pack new-pack-view.name :path new-pack-view.path)))

;; Existing packs matching what is typed (all of them when the field is
;; empty), so a pack name is picked rather than retyped.
(def pack-suggestions ()
  (let ((typed (string-downcase (string-trim region-view.pack))))
    (filter |name| (and (string-starts-with? (string-downcase name) typed)
                        (not (= (string-downcase name) typed)))
      (map |pack| (get pack :name) (seq-sample-packs "")))))

(def pack-entry ()
  (let ((suggestions (pack-suggestions)))
    (v-stack :width :fill :gap 0.2
      (text-input :key "sample-region-pack" :width :fill :height 1.35 :font-size 11
        :value region-view.pack :placeholder "e.g. training set 1"
        :on-change (lambda (v) (set! region-view.pack v)))
      (if (> (len suggestions) 0)
        (wrap :key "sample-region-pack-suggestions" :width :fill :gap 0.2 :row-gap 0.15 :align :center
          (each suggestions |name|
            (eseq.sample-import/suggestion-chip "sample-region-pack" name
              (lambda (picked) (set! region-view.pack picked)))))
        (box :width 0 :height 0 :bg :transparent)))))

;; The fields scroll and the buttons stay pinned below them: the modal is
;; clipped to its tile, and tags and suggestions grow the form.
(def panel ()
  (modal :key "sample-region-modal" :is-open region-view.open :on-close (lambda () (cancel))
    :title "Save Region" :width-px 640 :height-px 540
    (if region-view.open
    (v-stack :width :fill :height :fill :gap 0.6
      (scroll :key "sample-region-fields" :width :fill :flex 1
        (v-stack :width :fill :gap 0.6
          (label (str "From " region-view.parent " · " region-view.span)
            :key "sample-region-from" :bg :transparent :font-size 11 :color :dim
            :width :fill :wrap true)
          (v-stack :width :fill :gap 0.3
            (label "Name" :bg :transparent :font-size 11)
            (text-input :key "sample-region-name" :width :fill :height 1.35 :font-size 11
              :value region-view.name :placeholder "region name"
              :on-change (lambda (v) (set! region-view.name v))))
          (v-stack :width :fill :gap 0.3
            (label "Pack (optional)" :bg :transparent :font-size 11)
            (pack-entry))
          (v-stack :width :fill :gap 0.3
            (label "Tags" :bg :transparent :font-size 11)
            (eseq.sample-import/chip-row "sample-region-tags" region-view.tags
              (lambda (tag) (remove-tag tag)))
            (eseq.sample-import/tag-entry "sample-region-tag" region-view.tag-draft
              (lambda (v) (set! region-view.tag-draft v))
              region-view.tags
              (lambda (tag) (add-tag tag))
              "add a tag, Enter to apply"))
          (if (= region-view.error "") (box :height 0 :width 0)
            (label region-view.error :key "sample-region-error" :bg :transparent :font-size 11
              :color :red :width :fill :wrap true))))
      (h-stack :width :fill :gap 0.8 :align :center
        (box :height 0 :flex 1)
        (button "Cancel" :key "sample-region-cancel" :variant :ghost :on-click |event| (cancel))
        (button "Save Region" :key "sample-region-commit" :on-click |event| (commit))))
    (box :height 0 :width 0))))

(def new-pack-panel ()
  (modal :key "sample-new-pack-modal" :is-open new-pack-view.open
    :on-close (lambda () (close-new-pack))
    :title "New Pack" :width-px 480 :height-px 220
    (if new-pack-view.open
      (v-stack :width :fill :height :fill :gap 0.6
        (label "Packs collect samples for one purpose, like a training set, without adding tags."
          :bg :transparent :font-size 10 :color :dim)
        (text-input :key "sample-new-pack-name" :width :fill :height 1.35 :font-size 11
          :value new-pack-view.name :placeholder "pack name"
          :on-change (lambda (v) (set! new-pack-view.name v))
          :on-submit (lambda () (create-pack)))
        (box :width :fill :height 0 :flex 1)
        (h-stack :width :fill :gap 0.8 :align :center
          (box :height 0 :flex 1)
          (button "Cancel" :key "sample-new-pack-cancel" :variant :ghost
            :on-click |event| (close-new-pack))
          (button "Create and Add" :key "sample-new-pack-create"
            :on-click |event| (create-pack))))
      (box :height 0 :width 0))))
