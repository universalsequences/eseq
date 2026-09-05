"""Narrow regression tests for the offline identification objective.

uv run --with numpy --with scipy python -m unittest discover -s scripts -p test_grampian_tune.py
"""
import copy
import json
import unittest

import numpy as np
import grampian_tune as tuning


class GrampianAnalysisTests(unittest.TestCase):
    def setUp(self):
        path = tuning.ROOT / "crates/sequencer/tests/fixtures/spring/grampian-fit.json"
        self.params = json.loads(path.read_text())

    def test_packet_objective_does_not_reduce_to_a_time_centroid(self):
        # Each frequency occurs twice, symmetrically around 150 ms. Both
        # responses therefore have the SAME frequency-wise time centroid,
        # but only one has the intended chirp packet trajectories.
        t = np.arange(int(.31*tuning.SR))/tuning.SR
        reference = np.zeros_like(t)
        wrong = np.zeros_like(t)
        for hz in [1000, 2000, 3000, 4000, 5000]:
            offset = .025 + hz*.000008
            wrong_offset = .10-offset
            for target, distance in [(reference, offset), (wrong, wrong_offset)]:
                for center in [.15-distance, .15+distance]:
                    target += np.exp(-.5*((t-center)/.0015)**2)*np.cos(2*np.pi*hz*(t-center))
        objective = tuning.Objective(reference)
        self.assertLess(objective.components(reference)["packets"], 1e-12)
        self.assertGreater(objective.components(wrong)["packets"], .02)

    def test_silent_candidate_is_penalized_without_nan(self):
        ref = tuning.analytical(self.params)
        objective = tuning.Objective(ref)
        silent = objective.components(np.zeros_like(ref))
        self.assertTrue(all(np.isfinite(v) for v in silent.values()))
        self.assertGreater(objective.loss(np.zeros_like(ref)), objective.loss(ref)+1)

    def test_scattering_preserves_first_arrival_but_changes_returns(self):
        full = tuning.analytical(self.params, nfft=524288)
        bare = copy.deepcopy(self.params)
        for path in bare["paths"]:
            path["scatter_s"] = 0
        no_scatter = tuning.analytical(bare, nfft=524288)
        # No feedback can return in the first 25 ms; this catches accidental
        # placement of the scattering diffuser in front of the first pickup.
        self.assertLess(np.max(abs(full[:1200]-no_scatter[:1200])), 1e-5)
        self.assertGreater(np.linalg.norm(full[2400:14400]-no_scatter[2400:14400]), .01)

    def test_reference_manifest_checks_original_recording(self):
        ref, entry = tuning.reference()
        self.assertGreater(len(ref)/tuning.SR, 7)
        self.assertLess(np.sum(ref[:480]**2)/np.sum(ref**2), .02)
        self.assertEqual(entry["capture_family"], "grampian")


if __name__ == "__main__":
    unittest.main()
