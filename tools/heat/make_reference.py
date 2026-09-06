#!/usr/bin/env python3
"""Create isolated Analog measurement sets from an explicitly saved Live copy.

Never edits the input. All source clips and track automation are replaced in
copies; the instrument's complete parameter snapshot is recorded beside each
set. Export individual tracks in Live as 48 kHz, 32-bit float, without normalize,
return/master effects or looping. MIDI pitches are numeric (no octave labels).
"""
import argparse
import copy
import gzip
import json
import math
from pathlib import Path
import re
import xml.etree.ElementTree as ET


def value(root, path, val):
    node = root.find(path)
    if node is None:
        raise ValueError(f"Template is missing {path}")
    node.set("Value", str(val).lower() if isinstance(val, bool) else str(val))


def parameters(root, prefix=""):
    result = {}
    for child in root:
        manual = child.find("Manual")
        if manual is not None:
            result[prefix + child.tag] = manual.get("Value")
        else:
            result.update(parameters(child, prefix + child.tag + "/"))
    return result


def set_notes(clip, events):
    key_tracks = clip.find("Notes/KeyTracks")
    key_tracks.clear()
    notes_by_key = {}
    for i, event in enumerate(events):
        key = event["note"]
        velocity = event["velocity"]
        start = event["start_seconds"]
        duration = event["duration_seconds"]
        if not (isinstance(key, int) and 0 <= key <= 127
                and math.isfinite(velocity) and 1 <= velocity <= 127
                and math.isfinite(start) and math.isfinite(duration)
                and start >= 0 and duration > 0 and start + duration <= 20):
            raise ValueError(f"Invalid twenty-second measurement note: {event}")
        if key not in notes_by_key:
            kt = ET.SubElement(key_tracks, "KeyTrack", Id=str(len(notes_by_key)))
            notes_by_key[key] = ET.SubElement(kt, "Notes")
            ET.SubElement(kt, "MidiKey", Value=str(key))
        ns = notes_by_key[key]
        ET.SubElement(ns, "MidiNoteEvent", Time=str(start * 2), Duration=str(duration * 2),
                      Velocity=str(velocity), OffVelocity="64", NoteId=str(i + 1))
    clip.find("Notes/PerNoteEventStore/EventLists").clear()
    value(clip, "Notes/NoteIdGenerator/NextId", len(events) + 1)


def build(template, output, variants=None, sample_rate=48000, engine_sample_rate=48000):
    if template.resolve() == output.resolve():
        raise ValueError("Output must differ from the preserved input")
    root = ET.fromstring(gzip.decompress(template.read_bytes()))
    song = root.find("LiveSet")
    tracks = song.find("Tracks")
    source = next(t for t in tracks if t.tag == "MidiTrack" and t.find(".//UltraAnalog") is not None)
    track = copy.deepcopy(source)
    clip = copy.deepcopy(track.find(".//MidiClip"))
    if clip is None:
        raise ValueError("Template requires one MIDI clip to retain Live's clip schema")
    for t in list(tracks):
        if t.tag == "MidiTrack":
            tracks.remove(t)
    # No session/arrangement content or user automation enters the measurement.
    track.find("AutomationEnvelopes/Envelopes").clear()
    for slot in track.findall(".//ClipSlotList/ClipSlot/ClipSlot/Value"):
        slot.clear()
    take_lanes = track.find("TakeLanes/TakeLanes")
    if take_lanes is not None:
        take_lanes.clear()
    events = track.find("DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events")
    events.clear()
    clip.set("Time", "0")
    for path in ("CurrentEnd", "Loop/LoopEnd", "Loop/OutMarker", "Loop/HiddenLoopEnd"):
        value(clip, path, 40)
    for path in ("CurrentStart", "Loop/LoopStart", "Loop/StartRelative", "Loop/HiddenLoopStart"):
        value(clip, path, 0)
    value(clip, "Loop/LoopOn", False)
    value(clip, "Name", "Numeric MIDI pitch sequence")
    clip.find("Envelopes/Envelopes").clear()
    note_events = [dict(note=key, velocity=100, start_seconds=i * 4 + .5, duration_seconds=2)
                   for i, key in enumerate((24, 48, 60, 84, 108))]
    set_notes(clip, note_events)
    events.append(clip)
    value(track, "SavedPlayingSlot", -1)
    instrument = track.find(".//UltraAnalog")
    value(instrument, "SignalChain2/OscillatorToggle/Manual", False)
    value(instrument, "SignalChain2/AmplifierToggle/Manual", False)
    value(instrument, "SignalChain1/FilterToggle/Manual", False)
    value(instrument, "SignalChain1/FilterDrive/Manual", 0)
    value(instrument, "SignalChain1/FilterEnvCutoffMod/Manual", 0)
    value(instrument, "SignalChain1/FilterToFilter2/Manual", 0)
    value(instrument, "SignalChain1/Envelope.1/AmpMod/Manual", 0)
    value(instrument, "SignalChain1/Envelope.1/SustainLevel/Manual", 1)
    # Every parameter is explicit in the ledger; these are isolation settings,
    # not a claim that the source was an initialized factory preset.
    counter = int(song.find("NextPointeeId").get("Value"))
    cases = []
    if variants is None:
        variants = [dict(name=f"source-{wave}", overrides={"SignalChain1/OscillatorWaveShape": i})
                    for i, wave in enumerate(("sine", "saw", "rectangle", "white-noise"))]
    else:
        # Live's export alignment can differ between sets. Keep independent
        # unfiltered controls in each batch instead of fitting phase offsets
        # against a source rendered in a different session configuration.
        variants = list(variants) + [
            dict(name="calibration-saw", overrides={"SignalChain1/OscillatorWaveShape": 1}),
            dict(name="calibration-sine", overrides={"SignalChain1/OscillatorWaveShape": 0}),
        ]
    names = [variant["name"] for variant in variants]
    if not names or len(set(names)) != len(names) or any(
            re.fullmatch(r"[a-zA-Z0-9_.-]+", name) is None for name in names):
        raise ValueError("Measurement cases require unique, filename-safe names")
    for i, variant in enumerate(variants):
        test = copy.deepcopy(track)
        test.set("Id", str(100 + i))
        name = variant["name"]
        value(test, "Name/UserName", name)
        value(test, "Name/EffectiveName", name)
        inst = test.find(".//UltraAnalog")
        for path, setting in variant["overrides"].items():
            value(inst, path + "/Manual", setting)
        remap = {}
        for node in test.iter():
            if node.get("Id") is not None and (node.tag.endswith("Target")
                    or node.tag == "Pointee" or node.tag.startswith("ControllerTargets.")):
                old = node.get("Id")
                if old not in remap:
                    remap[old] = str(counter)
                    counter += 1
                node.set("Id", remap[old])
        for node in test.iter("PointeeId"):
            if node.get("Value") in remap:
                node.set("Value", remap[node.get("Value")])
        tracks.insert(i, test)
        case_notes = variant.get("notes", note_events)
        set_notes(test.find(".//MidiClip"), case_notes)
        cases.append(dict(name=name, parameters=parameters(inst), notes=case_notes))
    value(song, "NextPointeeId", counter)
    song.find("MainTrack/AutomationEnvelopes/Envelopes").clear()
    value(song, "MainTrack/DeviceChain/Mixer/Tempo/Manual", 120)
    value(song, "Transport/CurrentTime", 0)
    value(song, "Transport/LoopStart", 0)
    value(song, "Transport/LoopLength", 40)
    value(song, "SelectedDocumentViewInMainWindow", 0)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(gzip.compress(ET.tostring(root, encoding="utf-8", xml_declaration=True), mtime=0))
    output.with_suffix(".json").write_text(json.dumps(dict(live_version="12.4.5", sample_rate=sample_rate,
        engine_sample_rate=engine_sample_rate, tempo=120,
        duration_seconds=20, source_template=str(template), cases=cases), indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("template", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cases", type=Path, help="JSON array of named parameter overrides")
    parser.add_argument("--sample-rate", type=int, choices=(44100, 48000, 96000), default=48000,
                        help="Planned export rate recorded in the ledger; set this rate in Live's export dialog")
    parser.add_argument("--engine-sample-rate", type=int, choices=(44100, 48000, 96000), default=48000,
                        help="Planned engine rate; set this separately in Live's Audio Settings")
    args = parser.parse_args()
    if args.template.resolve() == args.output.resolve():
        parser.error("Output must differ from the preserved input")
    build(args.template, args.output, json.loads(args.cases.read_text()) if args.cases else None,
          args.sample_rate, args.engine_sample_rate)
