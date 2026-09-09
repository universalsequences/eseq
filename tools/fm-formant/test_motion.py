import copy
import json
import unittest
import tempfile
from pathlib import Path

from build_instrument import build, default_motion, validate_motion


class MotionValidation(unittest.TestCase):
    def test_native_asset_roundtrip(self):
        motion = default_motion()
        self.assertEqual(validate_motion(json.loads(json.dumps(motion))), motion)

    def test_four_operator_architecture(self):
        from presets import make_presets
        with tempfile.TemporaryDirectory() as directory:
            params = build(Path(directory), default_motion())
            tensor = json.loads((Path(directory) / 'motion-tensor.json').read_text())
            source = (Path(directory) / 'dsp.lisp').read_text()
        self.assertEqual(tensor['shape'], [5, 29])
        self.assertEqual(len(params), 186)
        self.assertNotIn('feedback_5', source)
        self.assertEqual(len([p for p in params if p.startswith('pm_')]), 6)
        self.assertEqual(len([p for p in params if p.startswith('fb_')]), 16)
        for preset in make_presets(params):
            self.assertEqual(set(preset['params']), set(params))
        old = default_motion()
        old['frames'][0]['pairs'] *= 2
        with self.assertRaises(ValueError):
            validate_motion(old)

    def test_nonclosed_motion_uses_one_shot_default(self):
        motion = default_motion()
        motion['frames'][-1]['pitch_semitones'] = 12
        with tempfile.TemporaryDirectory() as directory:
            params = build(Path(directory), motion)
        self.assertEqual(params['motion_mode'], 1)

    def test_rejects_invalid_data_before_generation(self):
        bad = [None, [], {}, {'version': 1, 'frames': []}]
        original = default_motion()
        for field, value in [('position', -.1), ('position', float('nan')),
                             ('pitch_semitones', float('inf')), ('pairs', 'invalid')]:
            motion = copy.deepcopy(original)
            motion['frames'][0][field] = value
            bad.append(motion)
        for field, value in [('voiced_center', 0), ('voiced_level', True),
                             ('skirt', 2), ('unvoiced_bandwidth', float('nan'))]:
            motion = copy.deepcopy(original)
            motion['frames'][0]['pairs'][0][field] = value
            bad.append(motion)
        motion = copy.deepcopy(original)
        motion['frames'][2]['position'] = motion['frames'][1]['position']
        bad.append(motion)
        for motion in bad:
            with self.subTest(motion=motion):
                with self.assertRaises(ValueError):
                    validate_motion(motion)


if __name__ == '__main__':
    unittest.main()
