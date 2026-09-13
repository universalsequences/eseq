import unittest
from analyze_profile import analyze


def node(key, start, end, worker):
    return {'id': key, 'logical_id': key, 'name': str(key), 'worker': worker,
            'start_ns': start*1000, 'end_ns': end*1000}


class ParallelProfileTests(unittest.TestCase):
    def test_parallel_work_is_not_added_to_callback_latency(self):
        result = analyze({'overflow': 0, 'frames': 512, 'workers': 4,
                          'start_ns': 0, 'end_ns': 12000,
                          'nodes': [node(0, 0, 3, 1), node(1, 0, 10, 2),
                                    node(2, 3, 8, 1), node(3, 10, 12, 1)],
                          'edges': [[0, 2], [2, 3], [1, 3]]})
        self.assertEqual(result['total_kernel_us'], 20)
        self.assertEqual(result['graph_wall_us'], 12)
        self.assertEqual(result['intrinsic_dependency_us'], 12)
        self.assertEqual([n['id'] for n in result['last_completion_chain']], [1, 3])

    def test_dispatch_wait_is_separate_from_kernel_cost(self):
        result = analyze({'overflow': 0, 'frames': 512, 'workers': 4,
                          'start_ns': 0, 'end_ns': 7000,
                          'nodes': [node(0, 2, 4, 1), node(1, 4, 6, 1)],
                          'edges': [[0, 1]]})
        self.assertEqual(result['intrinsic_dependency_us'], 4)
        self.assertEqual(result['last_completion_chain'][0]['ready_to_start_us'], 2)
        self.assertEqual(result['completion_to_return_us'], 1)

    def test_equal_timestamps_follow_dependencies_not_node_order(self):
        result = analyze({'overflow': 0, 'frames': 512, 'workers': 4,
                          'start_ns': 0, 'end_ns': 2000,
                          'nodes': [node(0, 0, 0, 1), node(1, 0, 0, 2),
                                    node(2, 0, 2, 1)],
                          'edges': [[1, 0], [0, 2]]})
        self.assertEqual(result['intrinsic_dependency_us'], 2)
        self.assertEqual([n['id'] for n in result['last_completion_chain']], [1, 0, 2])

    def test_rejects_incomplete_or_inconsistent_capture(self):
        with self.assertRaises(ValueError):
            analyze({'overflow': 1})
        with self.assertRaises(ValueError):
            analyze({'overflow': 0, 'frames': 512, 'workers': 4,
                     'start_ns': 0, 'end_ns': 7000,
                     'nodes': [node(0, 0, 4, 1), node(1, 3, 6, 2)],
                     'edges': [[0, 1]]})


if __name__ == '__main__':
    unittest.main()
