# AudioGraph

A real-time audio graph engine in C11 with lock-free multi-threaded scheduling, live graph editing, and zero-allocation audio processing.

- **Multi-threaded DAG scheduling** with Mach RT time-constraint promotion and OS Workgroup coordination
- **Live editing** — add, remove, and reconnect nodes while audio is playing
- **Lock-free throughout** — MPMC work queue, SPSC parameter ring, batched edit queue
- **Auto-summing** — multiple sources to one input transparently create hidden SUM nodes
- **Custom nodes** via function pointer vtable — bring your own DSP kernels
- **Swift integration** — ships as a `.dylib` with a C API ([Swift guide](SWIFT_INTEGRATION.md))

## Quick Start

```bash
make          # builds audiograph + libaudiograph.dylib
make test     # runs 29 test binaries (topology, concurrency, fuzz, stress)
```

```c
#include "graph_engine.h"
#include "graph_edit.h"

int main() {
    initialize_engine(512, 48000);
    LiveGraph *lg = create_live_graph(16, 512, "demo", 2); // stereo

    engine_enable_rt_time_constraint(1);
    engine_start_workers(4);

    // Build a signal chain
    int osc  = live_add_oscillator(lg, 440.0f, "A4");
    int gain = live_add_gain(lg, 0.3f, "vol");
    graph_connect(lg, osc, 0, gain, 0);
    graph_connect(lg, gain, 0, lg->dac_node_id, 0);  // left channel

    // Process audio
    float out[512 * 2];
    for (int i = 0; i < 100; i++)
        process_next_block(lg, out, 512);

    engine_stop_workers();
    destroy_live_graph(lg);
}
```

All graph operations (`graph_connect`, `add_node`, `delete_node`, etc.) are queued and applied atomically between audio blocks — safe to call from any thread.

## API Reference

### Engine

```c
void initialize_engine(int block_size, int sample_rate);
void engine_start_workers(int n);
void engine_stop_workers(void);
void engine_enable_rt_time_constraint(int enable); // Mach RT scheduling (macOS)
void engine_set_os_workgroup(void *oswg);           // OS Workgroup (macOS 10.16+)
```

### Graph Lifecycle

```c
LiveGraph *create_live_graph(int capacity, int block_size, const char *label, int channels);
void destroy_live_graph(LiveGraph *lg);
```

The `channels` parameter controls DAC topology: mono (1), stereo (2), etc. Output is interleaved: `[L0, R0, L1, R1, ...]`.

### Nodes

```c
// Generic — bring your own vtable
int add_node(LiveGraph *lg, NodeVTable vt, size_t state_size,
             const char *name, int nInputs, int nOutputs,
             const void *initial_state, size_t initial_state_size);
bool delete_node(LiveGraph *lg, int node_id);

// Built-in convenience functions
int live_add_oscillator(LiveGraph *lg, float freq_hz, const char *name);
int live_add_gain(LiveGraph *lg, float gain, const char *name);
int live_add_number(LiveGraph *lg, float value, const char *name);
int live_add_mixer2(LiveGraph *lg, const char *name);
int live_add_mixer8(LiveGraph *lg, const char *name);
int live_add_sum(LiveGraph *lg, const char *name, int nInputs);
```

All functions return a pre-allocated node ID immediately (atomic counter). The actual node creation is applied at the next block boundary.

### Connections

```c
bool graph_connect(LiveGraph *lg, int src, int src_port, int dst, int dst_port);
bool graph_disconnect(LiveGraph *lg, int src, int src_port, int dst, int dst_port);
```

Port-based: each node has numbered input/output ports. Connecting a second source to the same input port automatically creates a hidden SUM node.

### Processing

```c
void process_next_block(LiveGraph *lg, float *output, int nframes);
```

Call from your audio callback. Drains the edit queue, applies parameter updates, processes the DAG, and writes interleaved output. This is the only function that must run on the audio thread.

### Parameters

```c
bool params_push(ParamRing *r, ParamMsg m);
```

Lock-free parameter updates. Node state is a flat `float[]` array — `ParamMsg.idx` targets a specific index, `ParamMsg.logical_id` targets the node.

```c
ParamMsg msg = { .idx = 0, .logical_id = gain_id, .fvalue = 0.8f };
params_push(lg->params, msg);
```

### Hot Swap

```c
bool hot_swap_node(LiveGraph *lg, int node_id, NodeVTable vt, size_t state_size,
                   int nin, int nout, bool xfade, void (*migrate)(void*, void*),
                   const void *initial_state, size_t initial_state_size);

bool replace_keep_edges(LiveGraph *lg, int node_id, NodeVTable vt, size_t state_size,
                        int nin, int nout, bool xfade, void (*migrate)(void*, void*),
                        const void *initial_state, size_t initial_state_size);
```

`hot_swap_node` requires port-compatible signatures (can grow but not shrink connected ports). `replace_keep_edges` handles incompatible signatures by auto-disconnecting excess ports.

### Watch List

```c
bool add_node_to_watchlist(LiveGraph *lg, int node_id);
bool remove_node_from_watchlist(LiveGraph *lg, int node_id);
void *get_node_state(LiveGraph *lg, int node_id, size_t *size); // caller frees
```

State snapshots are captured after each block. Watched nodes (and their upstream dependencies) stay active even when disconnected from the DAC — useful for scope/analyzer nodes.

### Buffers

```c
int create_buffer(LiveGraph *lg, int size, int channels, const float *data);
int hot_swap_buffer(LiveGraph *lg, int id, const float *data, int size, int channels);
```

### Custom Nodes

Every node is a `NodeVTable` — a struct of function pointers:

```c
typedef void (*KernelFn)(float *const *in, float *const *out, int nframes,
                          void *state, void *buffers);

typedef struct {
    KernelFn  process;   // required: called every block
    InitFn    init;      // optional: called once after creation
    ResetFn   reset;     // optional: reset to initial state
    MigrateFn migrate;   // optional: copy state during hot-swap
} NodeVTable;
```

Example — a delay line with feedback:

```c
#define DELAY_BUF_SIZE 4096
#define DELAY_STATE_SIZE (DELAY_BUF_SIZE + 3) // buffer + time + feedback + write_pos

void delay_process(float *const *in, float *const *out, int n, void *state, void *buffers) {
    (void)buffers;
    float *s = (float *)state;
    float delay_time = s[0], feedback = s[1];
    int write_pos = (int)s[2];
    float *buf = &s[3];

    for (int i = 0; i < n; i++) {
        int read_pos = (write_pos - (int)delay_time + DELAY_BUF_SIZE) % DELAY_BUF_SIZE;
        float delayed = buf[read_pos];
        buf[write_pos] = in[0][i] + delayed * feedback;
        out[0][i] = in[0][i] + delayed * 0.5f;
        write_pos = (write_pos + 1) % DELAY_BUF_SIZE;
    }
    s[2] = (float)write_pos;
}

void delay_init(void *state, int sr, int max_block, const void *init_data) {
    float *s = (float *)state;
    s[0] = (float)sr / 4;  // 250ms
    s[1] = 0.3f;           // 30% feedback
}

const NodeVTable DELAY_VT = { .process = delay_process, .init = delay_init };

// Register:
int delay = add_node(lg, DELAY_VT, DELAY_STATE_SIZE * sizeof(float),
                     "delay", 1, 1, NULL, 0);
```

The `process` function must be **real-time safe**: no allocations, no locks, no syscalls.

## Design

### Threading Model

AudioGraph uses a **worker pool with Mach real-time scheduling** to parallelize DAG processing across cores.

The problem: **standard POSIX threads are worse than useless for parallel audio**. Under time-sharing scheduling, context switches and variable wakeup latency dominate. A graph that processes in 0.5ms on one RT thread can take 1.2ms on four normal threads:

| Configuration | Processing Time | Speedup |
|---|---|---|
| 1 RT thread | 0.48ms | 1.0x (baseline) |
| 4 normal threads | 1.15ms | 0.42x (slower!) |
| 4 RT threads | 0.18ms | 2.67x |
| 4 RT threads + OS Workgroup | 0.14ms | 3.43x |
| 8 RT threads + OS Workgroup | 0.09ms | 5.33x |

*Measured on M1, 64-node graph, 512-sample blocks @ 48kHz.*

The solution is three-layered:

1. **QoS hints** (`QOS_CLASS_USER_INTERACTIVE`) — always active on macOS, tells the scheduler this work is latency-sensitive
2. **Mach time-constraint policy** — promotes workers to hard real-time with a computation budget of 75% of the audio block period and a hard deadline at 100%. The OS guarantees no preemption within the budget.
3. **OS Workgroups** (macOS 11+) — workers join a shared workgroup so the kernel co-schedules them on separate cores, minimizes migration, and can boost/throttle the group as a unit

Workers park on a `pthread_cond_t` between blocks. When `process_next_block` is called, it broadcasts a wake, and all workers enter a spin-then-wait loop pulling nodes from the ready queue. The audio thread participates as a worker too — it doesn't just dispatch and wait.

### Block-Boundary Processing

All graph mutations are **deferred and batched**. The audio thread never sees a partially-modified graph.

Each call to `process_next_block` follows a strict sequence:

```
1. Drain edit queue    — apply all pending add/remove/connect/disconnect ops
2. Drain param ring    — apply parameter updates to node state arrays
3. Process DAG         — seed sources into ready queue, workers execute + fanout
4. Drain retire list   — free old state buffers from hot-swaps/deletions
5. Snapshot watch list — copy watched node states for external readers
```

Steps 1, 2, 4, and 5 may allocate memory (growing arrays, copying state). Step 3 — the actual audio processing — performs **zero allocations and acquires zero locks**. This is the real-time contract: the hot loop only does atomic decrements, queue pops, and DSP kernel calls.

### Lock-Free Queue Architecture

Three separate queues serve three different producer/consumer patterns:

| Queue | Type | Pattern | Purpose |
|---|---|---|---|
| **Ready queue** | Vyukov MPMC + semaphore | N producers, N consumers | Work distribution across worker threads |
| **Param ring** | SPSC ring buffer | 1 UI thread → 1 audio thread | Real-time parameter updates |
| **Edit queue** | SPSC ring buffer | 1 UI thread → 1 audio thread | Batched graph mutations |

The **MPMC ready queue** is the most interesting. It uses Vyukov's bounded queue with per-cell sequence numbers for ABA protection and 64-byte cache-line alignment per cell to eliminate false sharing. On top of the raw MPMC ring sits a `ReadyQ` wrapper that adds:
- An atomic length counter (so workers can fast-check "is there work?")
- A semaphore for timed waits (workers spin briefly, then park to reduce CPU burn)
- Batch push for seeding source nodes with a single semaphore signal instead of O(sources) signals

The param and edit queues are simpler SPSC rings — single-producer (UI thread), single-consumer (audio thread). They don't need CAS loops or sequence numbers.

### DAG Scheduling

The graph is a directed acyclic graph. Processing order is determined by a **topological sort via indegree tracking**:

1. **At edit time**: maintain an `indegree[]` array counting unique predecessors per node, and a `succ[]` list of successors per node
2. **At block start**: reset a `pending[]` array to `indegree[]` values. Seed all source nodes (indegree=0, has outputs) into the ready queue.
3. **After processing a node**: atomically decrement `pending[succ]` for each successor. When `pending` hits zero, that successor's inputs are all ready — push it to the ready queue.

This gives natural wavefront parallelism: independent branches of the DAG execute concurrently without explicit dependency analysis.

The scheduling metadata (source node list, job count, cycle detection) is **cached and only rebuilt when topology changes** — not every block. The `sched.dirty` flag tracks whether a connect/disconnect/delete invalidated the cache.

Orphaned nodes (not reachable from the DAC output) are detected via a reverse BFS from the DAC and excluded from scheduling entirely.

### Auto-Summing

When multiple sources connect to the same input port, AudioGraph transparently inserts a hidden SUM node:

```
osc1 ──┐
       ├── SUM ── gain
osc2 ──┘
```

The SUM node grows its input count as more sources connect and **collapses back to a direct connection** when reduced to one input. Users see logical point-to-point connections; the SUM management is fully automatic. SUM nodes are tracked per-port via `fanin_sum_node_id[]` so each input port on a node can independently have its own auto-sum.

### Edge Sharing & Free-List

When one output fans out to multiple destinations, all consumers **share the same buffer** — no copying. A reference count tracks consumers; the edge is retired when the last consumer disconnects.

Edge allocation uses an **intrusive free-list** threaded through the edge pool. `alloc_edge()` pops from the head in O(1); `retire_edge()` pushes back. This replaces what was previously an O(n) linear scan of the edge pool.

## Building

```bash
make                # executable + dylib
make debug          # with -g -O0
make test           # build and run all tests
make lib-release    # optimized dylib only (for integration)
```

The test suite includes 29 test binaries covering:
- Lock-free queue correctness (MPMC stress test with contention)
- Topology operations (connect, disconnect, auto-sum, orphan detection)
- Concurrency safety (hot-swap, deletion under load, deadlock prevention)
- Exhaustive fuzz testing (all 4-node edge permutations)
- Regression tests for specific bugs found during development

## License

MIT — see [COPYING](COPYING).
