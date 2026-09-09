"""Original voicings for the integration candidate, with complete scalar state."""
import json


def make_presets(defaults):
    presets = []
    families = ('Choir', 'Reed', 'Glass', 'Metal', 'Bass', 'Keys', 'Pad', 'Motion')
    for family in families:
        for variation in range(4):
            p = dict(defaults)
            p.update(motion_seconds=2+variation, formant_shift=(variation-1.5)*.12)
            if family in ('Choir','Pad','Motion'):
                p.update(motion_mode=2, motion_amount=1, breath=.15+.06*variation,
                         master_attack=100 if family=='Choir' else 500,
                         master_release=800 if family=='Choir' else 1800)
                if family=='Motion':
                    p.update(motion_seconds=.3+.27*variation, master_attack=15, fm_intensity=.7)
                    for i in range(1, 4):
                        p[f'pm_{i}_to_{i+1}'] = .3 + variation*.4
            elif family=='Reed':
                p.update(motion_amount=0, breath=.5, master_attack=8, master_release=160)
                for i in range(1,5):
                    p.update({f'v{i}_width':150+90*i, f'v{i}_center':300+500*i,
                              f'v{i}_freq_depth':.15, f'v{i}_freq_decay':80})
            else:
                p.update(motion_amount=0, breath=0, master_attack=2, master_release=400)
                for i in range(1,5):
                    p[f'v{i}_output'] = 1 if i%2==0 else 0
                    p[f'v{i}_mode'] = 0
                    p[f'v{i}_level'] = .8 if i%2==0 else .65
                for i in (1,3):
                    p[f'pm_{i}_to_{i+1}'] = 1.5 + variation*.6
                    p[f'v{i}_sustain'] = .03
                if family in ('Glass','Metal'):
                    p.update(master_sustain=0, master_decay=1800 if family=='Glass' else 700)
                    for i in range(1,5):
                        p[f'v{i}_ratio'] = ((1,2,5.43,3.01) if family=='Glass'
                                            else (1.414,1,5.33,2))[i-1]
                        p[f'v{i}_decay'] = 400+variation*250
                    if family=='Metal':
                        p['fb_4_to_1'] = .2+.25*variation
                elif family=='Bass':
                    p.update(master_sustain=.5, master_decay=180, master_release=100)
                    for i in range(1,5):
                        p[f'v{i}_ratio'] = .5 if i%2==0 else 1+variation
                        p[f'v{i}_decay'] = 80+40*variation
                elif family=='Keys':
                    p.update(master_sustain=.15, master_decay=1000)
                    for i in range(1,5):
                        p[f'v{i}_ratio'] = (1,1,3,2)[i-1]
                        p[f'v{i}_decay'] = 500+variation*100
            presets.append(dict(id=f'{family} {variation+1}', name=f'{family} {variation+1}',
                                base_note_offset=0, params=p, key_locks={}))
    for preset in presets:
        if set(preset['params']) != set(defaults):
            raise ValueError(f"Preset parameter mismatch: {preset['name']}")
    return presets


def write_bank(directory, defaults):
    bank = dict(version=1, engine_name='FM Formant', source_file='dsp.lisp',
                presets=make_presets(defaults))
    path = directory.parent / (directory.name+'.presets')
    path.write_text(json.dumps(bank, indent=2)+'\n')
    return bank
