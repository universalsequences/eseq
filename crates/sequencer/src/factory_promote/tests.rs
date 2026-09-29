use super::*;
use crate::project::{
    ProjectKitModConnection, ProjectKitPad, ProjectRackMacroMapping, ProjectRackRouting,
    ProjectRackSlotPattern, ProjectRackTrackSlot, ProjectSoundMetadata, ProjectTrack,
};

/// Factory = anything whose name starts with `f-`; samples are factory when
/// their stem starts with `f`.
struct Fake;

impl FactoryIndex for Fake {
    fn instrument(&self, id: &str) -> bool {
        id.starts_with("f-")
    }
    fn effect(&self, name: &str) -> bool {
        name.starts_with("builtin:") || name.starts_with("f-")
    }
    fn sample(&self, reference: &str) -> Option<String> {
        let stem = Path::new(reference).file_stem()?.to_str()?;
        stem.starts_with('f').then(|| format!("samples/{stem}.wav"))
    }
    fn filter_table(&self, reference: &str) -> bool {
        reference.starts_with("fltab:f-")
    }
    fn impulse(&self, reference: &str) -> bool {
        reference.starts_with("f-")
    }
    fn sequencer_source(&self, source: &str) -> bool {
        !source.contains("user")
    }
    fn instance_kind(&self, kind: &str) -> bool {
        kind.starts_with("factory/")
    }
}

fn custom_slot(name: &str) -> (ProjectRackTrackSlot, ProjectRackSlotPattern) {
    let mut pattern: ProjectRackSlotPattern = serde_json::from_value(serde_json::json!({
        "instrument_type": "custom",
    }))
    .unwrap();
    pattern.custom_effects = vec![None; 4];
    pattern.effect_slots = vec![ProjectEffectSlot::default(); 4];
    (
        ProjectRackTrackSlot {
            instrument_type: ProjectInstrumentType::Custom,
            sample_path: None,
            sample_name: None,
            instrument_name: Some(name.to_string()),
        },
        pattern,
    )
}

fn sampler_slot(path: &str) -> (ProjectRackTrackSlot, ProjectRackSlotPattern) {
    let (mut source, mut pattern) = custom_slot("");
    source.instrument_type = ProjectInstrumentType::Sampler;
    source.instrument_name = None;
    source.sample_path = Some(path.to_string());
    pattern.instrument_type = ProjectInstrumentType::Sampler;
    pattern.sample_path = Some(path.to_string());
    (source, pattern)
}

fn sound(slots: Vec<(ProjectRackTrackSlot, ProjectRackSlotPattern)>) -> ProjectSoundPreset {
    let (sources, patterns): (Vec<_>, Vec<_>) = slots.into_iter().unzip();
    ProjectSoundPreset {
        version: crate::project::project_file_version(),
        metadata: ProjectSoundMetadata { name: "S".into(), tags: Vec::new(), author: String::new() },
        track: ProjectTrack {
            id: crate::sequencer::TrackId(1),
            name: None,
            name_user_authored: false,
            color: None,
            collapsed: false,
            kind: ProjectTrackKind::Rack { routing: ProjectRackRouting::Broadcast, slots: sources },
        },
        rack: ProjectRackTrackPattern {
            routing: ProjectRackRouting::Broadcast,
            slots: patterns,
            macros: crate::project::default_project_rack_macros(),
        },
    }
}

fn map(target: ProjectRackMacroTarget) -> ProjectRackMacroMapping {
    ProjectRackMacroMapping { target, range_min: 0.0, range_max: 1.0, curve: Default::default() }
}

fn slot_count(sound: &ProjectSoundPreset) -> (usize, usize) {
    let ProjectTrackKind::Rack { slots, .. } = &sound.track.kind else { panic!("rack") };
    (slots.len(), sound.rack.slots.len())
}

#[test]
fn factory_sound_ships_whole() {
    let mut sound = sound(vec![custom_slot("f-synth")]);
    let mut report = PromoteReport::default();
    assert_eq!(sanitize_sound(&mut sound, &Fake, "", &mut report), SoundOutcome::Kept);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(slot_count(&sound), (1, 1));
}

#[test]
fn user_instrument_slot_is_skipped_and_macros_renumbered() {
    let mut sound = sound(vec![custom_slot("user-synth"), custom_slot("f-synth")]);
    sound.rack.macros[0].mappings = vec![
        map(ProjectRackMacroTarget::SlotParam { slot: 0, param: "gain".into() }),
        map(ProjectRackMacroTarget::SlotParam { slot: 1, param: "gain".into() }),
    ];
    let mut report = PromoteReport::default();
    assert_eq!(sanitize_sound(&mut sound, &Fake, "", &mut report), SoundOutcome::Kept);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].contains("user-synth"), "{:?}", report.skipped);
    assert_eq!(slot_count(&sound), (1, 1));
    let mappings = &sound.rack.macros[0].mappings;
    assert_eq!(mappings.len(), 1);
    assert!(matches!(mappings[0].target, ProjectRackMacroTarget::SlotParam { slot: 0, .. }));
}

#[test]
fn sound_with_nothing_factory_is_empty() {
    let mut sound = sound(vec![custom_slot("user-synth")]);
    let mut report = PromoteReport::default();
    assert_eq!(sanitize_sound(&mut sound, &Fake, "", &mut report), SoundOutcome::Empty);
}

#[test]
fn factory_samples_become_portable_and_user_samples_are_skipped() {
    let mut sound = sound(vec![
        sampler_slot("/Users/me/.local/samples/fabc.wav"),
        sampler_slot("samples/uxyz.wav"),
    ]);
    let mut report = PromoteReport::default();
    assert_eq!(sanitize_sound(&mut sound, &Fake, "", &mut report), SoundOutcome::Kept);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].contains("uxyz.wav"));
    let ProjectTrackKind::Rack { slots, .. } = &sound.track.kind else { panic!() };
    assert_eq!(slots[0].sample_path.as_deref(), Some("samples/fabc.wav"));
    assert_eq!(sound.rack.slots[0].sample_path.as_deref(), Some("samples/fabc.wav"));
}

#[test]
fn non_factory_effects_are_removed_and_the_chain_closes_up() {
    let (source, mut pattern) = custom_slot("f-synth");
    pattern.custom_effects = vec![
        Some("builtin:EQ8".into()),
        Some("shimmerpitch".into()),
        Some("builtin:Filter Table".into()),
        Some("builtin:Compressor".into()),
    ];
    pattern.effect_slots[2].table = Some("fltab:my-table".into());
    pattern.effect_slots[3].defaults = vec![0.5];
    let mut sound = sound(vec![(source, pattern)]);
    sound.rack.macros[0].mappings = vec![
        map(ProjectRackMacroTarget::SlotEffectParam {
            slot: 0, effect_slot: 1, param: "mix".into(), param_index: 0,
        }),
        map(ProjectRackMacroTarget::SlotEffectParam {
            slot: 0, effect_slot: 3, param: "ratio".into(), param_index: 0,
        }),
    ];
    let mut report = PromoteReport::default();
    sanitize_sound(&mut sound, &Fake, "", &mut report);
    assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
    let slot = &sound.rack.slots[0];
    assert_eq!(
        slot.custom_effects,
        vec![Some("builtin:EQ8".into()), Some("builtin:Compressor".into()), None, None]
    );
    assert_eq!(slot.effect_slots.len(), 4);
    assert_eq!(slot.effect_slots[1].defaults, vec![0.5]);
    let mappings = &sound.rack.macros[0].mappings;
    assert_eq!(mappings.len(), 1);
    assert!(matches!(
        mappings[0].target,
        ProjectRackMacroTarget::SlotEffectParam { slot: 0, effect_slot: 1, .. }
    ));
}

fn kit(pads: Vec<ProjectSoundPreset>) -> ProjectKitPreset {
    let mut kit: ProjectKitPreset = serde_json::from_value(serde_json::json!({
        "version": crate::project::project_file_version(),
        "metadata": { "name": "K" },
        "pads": [],
    }))
    .unwrap();
    kit.pads = pads
        .into_iter()
        .enumerate()
        .map(|(i, sound)| ProjectKitPad {
            pad_note: i as i32,
            choke_group: None,
            role: None,
            name: format!("pad{i}"),
            sound: Some(sound),
            modulator: None,
        })
        .collect();
    kit
}

#[test]
fn kit_drops_non_factory_pads_and_renumbers_cables() {
    let mut kit = kit(vec![
        sound(vec![custom_slot("user-kick")]),
        sound(vec![custom_slot("f-snare")]),
        sound(vec![custom_slot("f-hat")]),
    ]);
    kit.mod_connections = vec![
        ProjectKitModConnection {
            source_pad: 2,
            destination: ProjectKitModDestination::Pad(1),
            dest_input: 0,
        },
        ProjectKitModConnection {
            source_pad: 2,
            destination: ProjectKitModDestination::Pad(0),
            dest_input: 0,
        },
    ];
    kit.sequencers = vec![
        crate::project::ProjectRackSequencer {
            sequencer_id: 1,
            sequencer_name: "ok".into(),
            source: "(load \"@/scripts/a.lisp\")".into(),
        },
        crate::project::ProjectRackSequencer {
            sequencer_id: 2,
            sequencer_name: "mine".into(),
            source: "(load \"/user/b.lisp\")".into(),
        },
    ];
    let mut report = PromoteReport::default();
    sanitize_kit(&mut kit, &Fake, &mut report).unwrap();
    assert_eq!(kit.pads.len(), 2);
    assert_eq!(kit.pads[0].name, "pad1");
    assert_eq!(kit.mod_connections.len(), 1);
    assert_eq!(kit.mod_connections[0].source_pad, 1);
    assert_eq!(kit.mod_connections[0].destination, ProjectKitModDestination::Pad(0));
    assert_eq!(kit.sequencers.len(), 1);
    assert!(report.skipped.iter().any(|line| line.contains("pad 'pad0'")), "{:?}", report.skipped);
    assert!(report.skipped.iter().any(|line| line.contains("'mine'")), "{:?}", report.skipped);
}

#[test]
fn kit_with_no_factory_pads_is_refused() {
    let mut kit = kit(vec![sound(vec![custom_slot("user-kick")])]);
    assert!(sanitize_kit(&mut kit, &Fake, &mut PromoteReport::default()).is_err());
}

