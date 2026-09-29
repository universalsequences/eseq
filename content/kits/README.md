# Factory Kits

Read-only `.kit` presets shipped in the bundle and listed first in the
browser's Kits tab (`project::list_kit_presets` merges this dir with the user
tier). A kit is a drum rack config plus one Sound per pad and no patterns
(docs/drum-rack-v2-spec.md, "Polish"). Authoring the launch set is
eseq-2k9p.25.

To add one, select a pad of the drum rack in a dev checkout and run
`M-x promote-kit-to-factory`. Pads, bus effects and sequencers that depend on
non-factory content are listed in the modal and skipped. Promotion captures
pads and the bus chain only, not clips.
