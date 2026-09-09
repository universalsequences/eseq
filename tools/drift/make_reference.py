#!/usr/bin/env python3
"""Build isolated Drift captures from a preserved Live 12 set (never edited).

Uses the same clip/note construction as Heat, with Drift-specific isolation.
All native parameter values, including non-automatable selectors, are recorded.
"""
import argparse
import copy
import gzip
import hashlib
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.heat.make_reference import set_notes, value

LEVELS = [-24, -12, -6, -3, 0, 3, 6]
NOTES = [dict(note=k, velocity=100, start_seconds=.5 + 4*i, duration_seconds=2.5)
         for i, k in enumerate((36, 48, 60, 72, 84))]
ISOLATION = {
    'On': True, 'Filter_Frequency': 1000, 'Filter_Resonance': 0,
    'Filter_Type': 0, 'Filter_HiPassFrequency': 10, 'Filter_Tracking': 0,
    'Filter_ModAmount1': 0, 'Filter_ModAmount2': 0,
    'Filter_OscillatorThrough1': True, 'Filter_OscillatorThrough2': True,
    'Filter_NoiseThrough': True, 'Lfo_Amount': 0, 'Lfo_ModAmount': 0,
    'Oscillator1_Type': 0, 'Oscillator1_Shape': 0, 'Oscillator1_Transpose': 0,
    'Oscillator1_ShapeMod': 0, 'Oscillator2_Type': 0,
    'Oscillator2_Detune': 0, 'Oscillator2_Transpose': 0,
    'PitchModulation_Amount1': 0, 'PitchModulation_Amount2': 0,
    'Mixer_OscillatorGain1': .5, 'Mixer_OscillatorGain2': 0,
    'Mixer_OscillatorOn1': True, 'Mixer_OscillatorOn2': False,
    'Mixer_NoiseLevel': 0, 'Mixer_NoiseOn': False,
    'Envelope1_Attack': .001, 'Envelope1_Decay': .1,
    'Envelope1_Sustain': 1, 'Envelope1_Release': .05,
    'ModulationMatrix_Amount1': 0, 'ModulationMatrix_Amount2': 0,
    'ModulationMatrix_Amount3': 0, 'Global_VolVelMod': 0,
    'Global_ResetOscillatorPhase': True, 'Global_PolyVoiceDepth': 0,
    'Global_StereoVoiceDepth': 0, 'Global_UnisonVoiceDepth': 0,
    'Global_MonoVoiceDepth': 0, 'Global_DriftDepth': 0,
    'Global_Legato': False, 'Global_NotePitchBend': False,
    'Global_Glide': 0, 'Global_Volume': .25, 'Global_Transpose': 0,
}


def ledger(device):
    return {c.tag: (c.find('Manual').get('Value') if c.find('Manual') is not None
                    else c.get('Value')) for c in device
            if c.find('Manual') is not None or c.get('Value') is not None}


def cases(batch):
    result = []
    def add(name, **settings):
        result.append(dict(name=name, overrides=settings))
    for wave, enum in [('sine', 0), ('saw', 4)]:
        for db in (LEVELS if batch == 'levels' else [-24]):
            add(f'bypass-{wave}-{db:+d}', Oscillator1_Type=enum,
                Mixer_OscillatorGain1=10**(db/20), Filter_OscillatorThrough1=False)
    if batch == 'levels':
        for wave, enum in [('sine', 0), ('saw', 4)]:
            for typ in (0, 1):
                for res in (0, .8):
                    for db in LEVELS:
                        add(f't{typ+1}-{wave}-r{res:g}-{db:+d}', Oscillator1_Type=enum,
                            Filter_Type=typ, Filter_Resonance=res, Mixer_OscillatorGain1=10**(db/20))
        for typ in (0, 1):
            for vol in (.125, .5):
                add(f'volume-t{typ+1}-{vol:g}', Oscillator1_Type=4, Filter_Type=typ,
                    Filter_Resonance=.8, Mixer_OscillatorGain1=10**(6/20), Global_Volume=vol)
        add('repeat-bypass-sine', Filter_OscillatorThrough1=False, Mixer_OscillatorGain1=10**(-24/20))
        add('repeat-t1-saw', Oscillator1_Type=4, Filter_Resonance=.8, Mixer_OscillatorGain1=10**(6/20))
    elif batch == 'response':
        for typ in (0, 1):
            for cutoff in (250, 1000, 4000):
                for res in (0, .5, .8, 1):
                    add(f'response-t{typ+1}-f{cutoff}-r{res:g}', Oscillator1_Type=4,
                        Filter_Type=typ, Filter_Frequency=cutoff, Filter_Resonance=res,
                        Mixer_OscillatorGain1=10**(-24/20))
        for hp in (100, 1000, 4000):
            add(f'hp-{hp}', Oscillator1_Type=4, Filter_Frequency=19999,
                Filter_HiPassFrequency=hp, Mixer_OscillatorGain1=10**(-24/20))
        for typ in (0, 1):
            for db in (-6, 0, 6):
                add(f'mixed-t{typ+1}-{db:+d}', Filter_Type=typ, Filter_Resonance=.8,
                    Mixer_OscillatorGain1=10**(db/20), Mixer_OscillatorOn2=True,
                    Mixer_OscillatorGain2=10**(db/20), Oscillator2_Detune=7)
        for db in (-6, 0, 6):
            add(f'mixed-bypass-{db:+d}', Filter_OscillatorThrough1=False,
                Filter_OscillatorThrough2=False, Mixer_OscillatorGain1=10**(db/20),
                Mixer_OscillatorOn2=True, Mixer_OscillatorGain2=10**(db/20), Oscillator2_Detune=7)
    elif batch == 'summing':
        for db in (-6, 0, 6):
            for vol in (.125, .25, .5):
                add(f'sum-{db:+d}-v{vol:g}', Filter_OscillatorThrough1=False,
                    Filter_OscillatorThrough2=False, Mixer_OscillatorGain1=10**(db/20),
                    Mixer_OscillatorOn2=True, Mixer_OscillatorGain2=10**(db/20),
                    Oscillator2_Detune=7, Global_Volume=vol)
            add(f'solo2-{db:+d}', Filter_OscillatorThrough2=False, Mixer_OscillatorOn1=False,
                Mixer_OscillatorOn2=True, Mixer_OscillatorGain2=10**(db/20), Oscillator2_Detune=7)
        for typ in (0,1):
            for vol in (.125,.25,.5):
                add(f'filtered-sum-t{typ+1}-v{vol:g}', Filter_Type=typ, Filter_Resonance=.8,
                    Mixer_OscillatorGain1=10**(6/20), Mixer_OscillatorOn2=True,
                    Mixer_OscillatorGain2=10**(6/20), Oscillator2_Detune=7, Global_Volume=vol)
    elif batch == 'linear':
        for db in (-48,-60):
            add(f'bypass-saw-{db}', Oscillator1_Type=4,
                Mixer_OscillatorGain1=10**(db/20), Filter_OscillatorThrough1=False)
            for typ in (0,1):
                for cutoff in (250,1000,4000):
                    for res in (0,.5,.8,.95):
                        add(f'linear-t{typ+1}-f{cutoff}-r{res:g}-{db}',
                            Oscillator1_Type=4, Filter_Type=typ, Filter_Frequency=cutoff,
                            Filter_Resonance=res, Mixer_OscillatorGain1=10**(db/20))
        for sustain in (.25,.5,1):
            add(f'amp-placement-{sustain:g}', Filter_OscillatorThrough1=False,
                Filter_OscillatorThrough2=False, Mixer_OscillatorGain1=10**(6/20),
                Mixer_OscillatorOn2=True, Mixer_OscillatorGain2=10**(6/20),
                Oscillator2_Detune=7, Envelope1_Sustain=sustain)
    elif batch == 'sine-response':
        for cutoff,center in ((250,48),(1000,72),(4000,96)):
            notes = [dict(n,note=center+offset) for n,offset in zip(NOTES,(-24,-12,-2,2,12))]
            for db in (-24,-36):
                add(f'bypass-sine-f{cutoff}-{db}', Mixer_OscillatorGain1=10**(db/20),
                    Filter_OscillatorThrough1=False)
                result[-1]['notes'] = notes
                for typ in (0,1):
                    for res in (0,.5,.8,.95):
                        add(f'sine-t{typ+1}-f{cutoff}-r{res:g}-{db}', Filter_Type=typ,
                            Filter_Frequency=cutoff, Filter_Resonance=res,
                            Mixer_OscillatorGain1=10**(db/20))
                        result[-1]['notes'] = notes
    elif batch == 'resonance-law':
        # Sample both sides of the 1 kHz peak directly. The earlier law batch
        # had only one fundamental near the peak, leaving pole fits ambiguous.
        notes = [dict(n,note=pitch) for n,pitch in zip(NOTES,(76,80,83,86,90))]
        for db in (-48,-60):
            add(f'bypass-law-{db}', Mixer_OscillatorGain1=10**(db/20),
                Filter_OscillatorThrough1=False)
            result[-1]['notes'] = notes
            settings = [i/20 for i in range(21)] if db == -48 else [.8,.9,.95,1]
            for res in settings:
                add(f'law-t2-r{res:g}-{db}', Filter_Type=1, Filter_Frequency=1000,
                    Filter_Resonance=res, Mixer_OscillatorGain1=10**(db/20))
                result[-1]['notes'] = notes
    elif batch == 'character':
        for wave, enum in [('sine',0),('saw',4)]:
            for db in (0,6):
                add(f'bypass-{wave}-{db:+d}', Oscillator1_Type=enum,
                    Mixer_OscillatorGain1=10**(db/20), Filter_OscillatorThrough1=False)
                for typ in (0,1):
                    for cutoff in (250,4000):
                        for res in (0,.5,.8):
                            add(f'character-t{typ+1}-{wave}-f{cutoff}-r{res:g}-{db:+d}',
                                Oscillator1_Type=enum, Filter_Type=typ,
                                Filter_Frequency=cutoff, Filter_Resonance=res,
                                Mixer_OscillatorGain1=10**(db/20))
        notes = [dict(n,note=72+offset) for n,offset in zip(NOTES,(-24,-12,-2,2,12))]
        add('bypass-sine-law--36', Mixer_OscillatorGain1=10**(-36/20),
            Filter_OscillatorThrough1=False)
        result[-1]['notes'] = notes
        for res in (0,.1,.2,.3,.4,.5,.6,.7,.8,.9,.95,1):
            add(f'law-t2-r{res:g}', Filter_Type=1, Filter_Frequency=1000,
                Filter_Resonance=res, Mixer_OscillatorGain1=10**(-36/20))
            result[-1]['notes'] = notes
    elif batch == 'highpass-drive':
        for db in (-6,0,6):
            add(f'bypass-sine-{db:+d}', Mixer_OscillatorGain1=10**(db/20),
                Filter_OscillatorThrough1=False)
        for typ in (0,1):
            for hp in (20,1000,4000):
                for db in (-24,-6,0,6):
                    add(f'hp-drive-t{typ+1}-h{hp}-{db:+d}', Filter_Type=typ,
                        Filter_Frequency=1000, Filter_Resonance=0,
                        Filter_HiPassFrequency=hp, Mixer_OscillatorGain1=10**(db/20))
    elif batch == 'highpass':
        notes = [dict(n,note=pitch) for n,pitch in zip(NOTES,(12,24,36,48,60))]
        for db in (-24,-36):
            add(f'bypass-sine-low-{db}', Mixer_OscillatorGain1=10**(db/20),
                Filter_OscillatorThrough1=False)
            result[-1]['notes'] = notes
            for typ in (0,1):
                for hp in (10,15,20,30,40,100):
                    add(f'highpass-t{typ+1}-h{hp}-{db}', Filter_Type=typ,
                        Filter_Frequency=19999, Filter_Resonance=0,
                        Filter_HiPassFrequency=hp, Mixer_OscillatorGain1=10**(db/20))
                    result[-1]['notes'] = notes
    elif batch == 'ordering':
        # Break the confounding between pitch, time, and voice allocation in
        # the ascending-note captures. Each loud case has its own quiet control.
        for order, pitches in [('ascending', (36,48,60,72,84)),
                              ('descending', (84,72,60,48,36)),
                              ('low-repeat', (36,)*5), ('high-repeat', (84,)*5)]:
            for db in (-6,6):
                add(f'{order}-{db:+d}', Filter_OscillatorThrough1=False,
                    Filter_OscillatorThrough2=False, Mixer_OscillatorGain1=10**(db/20),
                    Mixer_OscillatorOn2=True, Mixer_OscillatorGain2=10**(db/20),
                    Oscillator2_Detune=7)
                result[-1]['notes'] = [dict(n, note=pitch) for n,pitch in zip(NOTES,pitches)]
    else:
        raise ValueError(batch)
    return result


def build(template, output, batch):
    if output.exists() or output.with_suffix('.json').exists():
        raise ValueError(f'Refusing to overwrite existing capture set/ledger: {output}')
    data = template.read_bytes()
    root = ET.fromstring(gzip.decompress(data))
    song = root.find('LiveSet')
    tracks = song.find('Tracks')
    sources = [t for t in tracks if t.find('DeviceChain/DeviceChain/Devices/Drift') is not None]
    if len(sources) != 1:
        raise ValueError('Template must contain exactly one track with a top-level Drift')
    track = copy.deepcopy(sources[0])
    clip_source = track.find('.//MidiClip')
    if clip_source is None:
        raise ValueError('Drift track needs a MIDI clip as a schema template')
    clip = copy.deepcopy(clip_source)
    tracks.clear()
    track.find('AutomationEnvelopes/Envelopes').clear()
    for slot in track.findall('.//ClipSlotList/ClipSlot/ClipSlot/Value'):
        slot.clear()
    takes = track.find('TakeLanes/TakeLanes')
    if takes is not None:
        takes.clear()
    events = track.find('DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events')
    events.clear()
    clip.set('Time', '0')
    for path in ('CurrentEnd', 'Loop/LoopEnd', 'Loop/OutMarker', 'Loop/HiddenLoopEnd'):
        value(clip, path, 40)
    for path in ('CurrentStart', 'Loop/LoopStart', 'Loop/StartRelative', 'Loop/HiddenLoopStart'):
        value(clip, path, 0)
    value(clip, 'Loop/LoopOn', False)
    value(clip, 'Name', 'Drift isolated reference')
    clip.find('Envelopes/Envelopes').clear()
    set_notes(clip, NOTES)
    events.append(clip)
    value(track, 'SavedPlayingSlot', -1)
    devices = track.find('DeviceChain/DeviceChain/Devices')
    drift = devices.find('Drift')
    devices.clear()
    devices.append(drift)
    for key, setting in ISOLATION.items():
        value(drift, key+'/Manual', setting)
    value(drift, 'Global_VoiceMode', 0)
    value(drift, 'Global_HiQuality', True)
    mixer = track.find('DeviceChain/Mixer')
    mixer.find('Sends').clear()
    value(mixer, 'Speaker/Manual', True)
    value(mixer, 'SoloSink', False)
    value(mixer, 'Pan/Manual', 0)
    value(mixer, 'Volume/Manual', 1)
    value(track, 'DeviceChain/AudioOutputRouting/Target', 'AudioOut/Main')
    # Record arm is a direct leaf in Live's main sequencer.
    value(track, 'DeviceChain/MainSequencer/Recorder/IsArmed', False)
    counter = int(song.find('NextPointeeId').get('Value'))
    records = []
    for i, case in enumerate(cases(batch)):
        test = copy.deepcopy(track)
        test.set('Id', str(100+i))
        name = case['name'].replace('+', 'p')
        value(test, 'Name/UserName', name)
        value(test, 'Name/EffectiveName', name)
        inst = test.find('.//Drift')
        for key, setting in case['overrides'].items():
            value(inst, key+'/Manual', setting)
        notes = case.get('notes', NOTES)
        if 'notes' in case:
            set_notes(test.find('.//MidiClip'), notes)
        remap = {}
        for node in test.iter():
            if node.get('Id') is not None and (node.tag.endswith('Target') or node.tag == 'Pointee'
                                               or node.tag.startswith('ControllerTargets.')):
                old = node.get('Id')
                if old not in remap:
                    remap[old] = str(counter)
                    counter += 1
                node.set('Id', remap[old])
        for node in test.iter('PointeeId'):
            if node.get('Value') in remap:
                node.set('Value', remap[node.get('Value')])
        tracks.append(test)
        records.append(dict(name=name, parameters=ledger(inst), notes=notes))
    value(song, 'NextPointeeId', counter)
    main = song.find('MainTrack')
    main.find('AutomationEnvelopes/Envelopes').clear()
    main.find('DeviceChain/DeviceChain/Devices').clear()
    value(main, 'DeviceChain/Mixer/Tempo/Manual', 120)
    value(main, 'DeviceChain/Mixer/Volume/Manual', 1)
    value(main, 'DeviceChain/Mixer/Pan/Manual', 0)
    for path, setting in [('Transport/CurrentTime',0),('Transport/LoopStart',0),
                          ('Transport/LoopLength',40),('SelectedDocumentViewInMainWindow',0)]:
        value(song, path, setting)
    output.parent.mkdir(parents=True, exist_ok=True)
    encoded = gzip.compress(ET.tostring(root, encoding='utf-8', xml_declaration=True), mtime=0)
    output.write_bytes(encoded)
    output.with_suffix('.json').write_text(json.dumps(dict(live_version=root.get('Creator'),
        sample_rate=48000, engine_sample_rate=48000, duration_seconds=20, tempo=120,
        source_template=str(template.resolve()), source_sha256=hashlib.sha256(data).hexdigest(),
        set_sha256=hashlib.sha256(encoded).hexdigest(), batch=batch, cases=records), indent=2)+'\n')
    print(f'{output}: {len(records)} isolated tracks')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('template', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--batch', choices=['levels','response','summing','linear','ordering','sine-response','highpass','highpass-drive','character','resonance-law'], required=True)
    args = parser.parse_args()
    build(args.template, args.output, args.batch)
