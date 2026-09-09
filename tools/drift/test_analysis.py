"""Numerical checks for measurements, independent of Live or the synth model."""
import gzip
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from analyze_reference import SR, analyze, capture_bytes, db, fundamental, harmonics, load
from diagnose_reference import fit_ceiling, harmonic_noise, relative_error
from make_reference import NOTES, cases
from fit_linear import response


class AnalysisTests(unittest.TestCase):
    def test_character_and_drive_order_have_matching_excitation_controls(self):
        for batch in ('character','highpass-drive','resonance-law'):
            controls = cases(batch)
            self.assertEqual(len({c['name'] for c in controls}),len(controls))
            for case in controls:
                p = case['overrides']
                if p.get('Filter_OscillatorThrough1') is False:
                    continue
                matches = [b for b in controls
                           if b['overrides'].get('Filter_OscillatorThrough1') is False
                           and b.get('notes',NOTES)==case.get('notes',NOTES)
                           and b['overrides'].get('Oscillator1_Type',0)==p.get('Oscillator1_Type',0)
                           and b['overrides']['Mixer_OscillatorGain1']==p['Mixer_OscillatorGain1']]
                self.assertEqual(len(matches),1)

    def test_bypass_parameters_identify_custom_pitch_control_without_name_convention(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            (folder/'test.als').write_bytes(b'synthetic test set')
            params = dict(Oscillator1_Type='0', Filter_OscillatorThrough1='false',
                          Mixer_OscillatorOn2='false', Mixer_NoiseOn='false',
                          Mixer_OscillatorGain1='0.1')
            entries = []
            for name, note, scale, through in [('bypass-sine--24',36,1,'false'),
                                              ('custom-control',76,1,'false'),
                                              ('filtered',76,.5,'true')]:
                event = dict(note=note,start_seconds=.5,duration_seconds=2.5)
                p = dict(params,Filter_OscillatorThrough1=through)
                entries.append(dict(name=name,notes=[event],parameters=p))
                f = 440*2**((note-69)/12)
                x = (.01*scale*np.sin(2*np.pi*f*np.arange(20*SR)/SR)).astype(np.float32)
                wavfile.write(folder/f'test {name}.wav',SR,np.column_stack((x,x)))
            ledger = dict(cases=entries,batch='synthetic',live_version='synthetic',
                          source_sha256='synthetic',
                          set_sha256=hashlib.sha256((folder/'test.als').read_bytes()).hexdigest())
            (folder/'test.json').write_text(json.dumps(ledger))
            report = analyze(folder/'test.json')
            comparison = report['spectral_ratios']['filtered']
            self.assertEqual(comparison['bypass'],'custom-control')
            self.assertAlmostEqual(comparison['notes'][0]['output_to_bypass_db'][0],
                                   20*np.log10(.5),places=5)

    def test_sine_response_has_matching_bypass_for_each_excitation(self):
        for batch, prefix in [('sine-response','sine-t'), ('highpass','highpass-t')]:
            controls = cases(batch)
            for case in controls:
                if case['name'].startswith(prefix):
                    matches = [b for b in controls if b['name'].startswith('bypass-sine')
                               and b.get('notes')==case['notes']
                               and b['overrides']['Mixer_OscillatorGain1']==case['overrides']['Mixer_OscillatorGain1']]
                    self.assertEqual(len(matches),1)

    def test_resonant_section_has_expected_corner_gain_and_phase(self):
        for rate in (48000,96000):
            low = response(np.array([1000.]),rate,1000,.7)[0]
            high = response(np.array([1000.]),rate,1000,.7,True)[0]
            self.assertAlmostEqual(abs(low),.7,places=12)
            self.assertAlmostEqual(np.angle(low),-np.pi/2,places=12)
            self.assertAlmostEqual(abs(high),.7,places=12)
            self.assertAlmostEqual(np.angle(high),np.pi/2,places=12)

    def test_clipping_fit_predicts_independent_input(self):
        t = np.arange(SR)/SR
        x = .6*np.sin(2*np.pi*67*t)+.5*np.sin(2*np.pi*101*t)
        ceiling = fit_ceiling(x,np.clip(x,-.4,.4),'clip')
        self.assertAlmostEqual(ceiling,.4,places=7)
        holdout = .8*np.sin(2*np.pi*419*t)+.3*np.sin(2*np.pi*557*t)
        self.assertLess(relative_error(np.clip(holdout,-ceiling,ceiling),
                                      np.clip(holdout,-.4,.4)),1e-7)

    def test_local_noise_distinguishes_weak_harmonic(self):
        t = np.arange(SR)/SR
        x = .2*np.sin(2*np.pi*130.8128*t)+.01*np.random.default_rng(731).normal(size=SR)
        h,noise = harmonic_noise(x,130.8128)
        self.assertGreater(float(db(h[0]/noise[0])),50)
        self.assertLess(float(db(h[2]/noise[2])),20)

    def test_ordering_controls_pair_notes_and_levels(self):
        controls = {c['name']:c for c in cases('ordering')}
        for order,notes in [('ascending',[36,48,60,72,84]),
                            ('descending',[84,72,60,48,36]),
                            ('low-repeat',[36]*5),('high-repeat',[84]*5)]:
            low,high = controls[order+'--6'],controls[order+'-+6']
            self.assertEqual(low['notes'],high['notes'])
            self.assertEqual([n['note'] for n in low['notes']],notes)
            self.assertAlmostEqual(high['overrides']['Mixer_OscillatorGain1']/
                                   low['overrides']['Mixer_OscillatorGain1'],10**.6)

    def test_off_bin_harmonics_recover_known_amplitudes(self):
        t = np.arange(SR)/SR
        f = 130.8128
        x = .2*np.sin(2*np.pi*f*t+.3) + .02*np.sin(2*np.pi*3*f*t-1.1)
        measured = fundamental(x, f*1.0001)
        self.assertLess(abs(measured-f), 1e-5)
        h = harmonics(x, measured)
        self.assertAlmostEqual(h[0], .2, places=6)
        self.assertAlmostEqual(h[2], .02, places=6)
        self.assertAlmostEqual(float(db(np.linalg.norm(h[1:])/h[0])), -20, places=3)

    def test_linear_gain_preserves_spectral_ratios(self):
        t = np.arange(SR)/SR
        x = np.sin(2*np.pi*261.6256*t) + .3*np.sin(2*np.pi*523.2512*t)
        a, b = harmonics(x,261.6256),harmonics(x*.25,261.6256)
        np.testing.assert_allclose(db(b[:2]/a[:2]), 20*np.log10(.25), atol=1e-10)

    def test_capture_validation_rejects_wrong_duration_and_nonfinite(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder)/'capture.wav'
            wavfile.write(path,SR,np.ones((SR,2),dtype=np.float32))
            with self.assertRaisesRegex(ValueError,'expected 20 s'):
                load(path)
            y = np.ones((20*SR,2),dtype=np.float32)
            y[0,0] = np.nan
            wavfile.write(path,SR,y)
            with self.assertRaisesRegex(ValueError,'nonfinite'):
                load(path)

    def test_compressed_capture_retains_exact_float_samples_and_bytes(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder)/'capture.wav'
            y = np.full((20*SR,2), .125, dtype=np.float32)
            wavfile.write(path, SR, y)
            raw = path.read_bytes()
            path.with_suffix('.wav.gz').write_bytes(gzip.compress(raw, mtime=0))
            path.unlink()
            self.assertEqual(capture_bytes(path), raw)
            np.testing.assert_array_equal(load(path), y)


if __name__ == '__main__':
    unittest.main()
