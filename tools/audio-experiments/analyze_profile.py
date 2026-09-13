#!/usr/bin/env python3
"""Analyze parallel graph captures without treating summed DSP time as latency."""
import argparse
from graphlib import TopologicalSorter
import json
from pathlib import Path
import statistics


def analyze(capture):
    if capture['overflow']:
        raise ValueError('Graph profile exceeded its node or edge capacity')
    start, end = capture['start_ns'], capture['end_ns']
    nodes = {n['id']: n for n in capture['nodes']}
    predecessors = {key: set() for key in nodes}
    for source, destination in capture['edges']:
        if source in nodes and destination in nodes:
            predecessors[destination].add(source)
    longest, longest_parent, gating_parent, waits = {}, {}, {}, {}
    workers = {}
    # Small nodes can begin and end on the same clock tick. Timestamp sorting
    # cannot order those dependencies; use the recorded DAG itself.
    for key in TopologicalSorter(predecessors).static_order():
        n = nodes[key]
        if not start <= n['start_ns'] <= n['end_ns'] <= end:
            raise ValueError(f'Invalid timestamp bounds for node {key}')
        parents = predecessors[key]
        if any(nodes[p]['end_ns'] > n['start_ns'] or p not in longest for p in parents):
            raise ValueError(f'Dependency ordering violated at node {key}')
        parent = max(parents, key=lambda p: longest[p], default=None)
        gating = max(parents, key=lambda p: nodes[p]['end_ns'], default=None)
        duration = n['end_ns'] - n['start_ns']
        longest[key] = duration + (longest[parent] if parent is not None else 0)
        longest_parent[key], gating_parent[key] = parent, gating
        ready = nodes[gating]['end_ns'] if gating is not None else start
        waits[key] = n['start_ns'] - ready
        workers[n['worker']] = workers.get(n['worker'], 0) + duration

    def chain(key, parent):
        result = []
        while key is not None:
            n = nodes[key]
            result.append({'id': key, 'name': n['name'], 'worker': n['worker'],
                           'duration_us': (n['end_ns']-n['start_ns'])/1000,
                           'ready_to_start_us': waits[key]/1000})
            key = parent[key]
        return result[::-1]

    last = max(nodes, key=lambda key: nodes[key]['end_ns'], default=None)
    intrinsic = max(longest, key=longest.get, default=None)
    return {'frames': capture['frames'], 'helper_workers': capture['workers'],
            'graph_wall_us': (end-start)/1000,
            'total_kernel_us': sum(workers.values())/1000,
            'worker_busy_us': {key: value/1000 for key, value in workers.items()},
            'intrinsic_dependency_us': longest.get(intrinsic, 0)/1000,
            'intrinsic_dependency_chain': chain(intrinsic, longest_parent),
            'last_completion_chain': chain(last, gating_parent),
            'completion_to_return_us': (end-nodes[last]['end_ns'])/1000 if last is not None else 0}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profile', type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    captures = [json.loads(line) for line in args.profile.read_text().splitlines() if line.strip()]
    if not captures:
        raise ValueError('No graph captures recorded')
    results = [analyze(c) for c in captures]
    middle = sorted(range(len(results)), key=lambda i: results[i]['graph_wall_us'])[len(results)//2]
    summary = {'captures': len(results),
               'median_graph_wall_us': statistics.median(r['graph_wall_us'] for r in results),
               'median_total_kernel_us': statistics.median(r['total_kernel_us'] for r in results),
               'median_intrinsic_dependency_us': statistics.median(r['intrinsic_dependency_us'] for r in results),
               'representative_capture_index': middle,
               'representative_capture': results[middle], 'captures_detail': results}
    args.out.write_text(json.dumps(summary, indent=2)+'\n')
    # Standard Chrome/Perfetto trace, entirely local. Each process is a captured
    # slice; thread zero is the callback and positive slots are helper workers.
    events = []
    for index, capture in enumerate(captures):
        for n in capture['nodes']:
            events.append({'name': n['name'], 'cat': 'DSP', 'ph': 'X', 'pid': index,
                           'tid': n['worker'], 'ts': (n['start_ns']-capture['start_ns'])/1000,
                           'dur': (n['end_ns']-n['start_ns'])/1000,
                           'args': {'node_id': n['id'], 'logical_id': n['logical_id']}})
    args.out.with_suffix('.trace.json').write_text(json.dumps({'traceEvents': events}))
    print(json.dumps({k: v for k, v in summary.items() if k != 'captures_detail'}, indent=2))


if __name__ == '__main__':
    main()
