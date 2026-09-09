# Factory Sounds

Read-only `.sound` presets shipped in the bundle and listed first in the
browser's Sounds tab (`project::list_sound_presets` merges this dir with the
user tier in `~/.local/sounds` / Application Support). Save from the app writes
to the user tier only, so nothing here can be overwritten in place.

Each file is a `ProjectSoundPreset` JSON, the same shape the Sounds tab's
"save sound" flow writes. Instrument references must be factory-relative
(`core/triton/`, `instruments/Synths/...`) so they resolve inside the DMG.
Authoring the launch set is eseq-2k9p.25.
