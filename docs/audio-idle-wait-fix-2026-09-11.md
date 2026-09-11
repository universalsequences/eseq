# Audio graph completion and idle waiting

Date: 2026-09-11. Issues: eseq-j67n, eseq-z75r.

This follows the [initial worker experiments](audio-worker-experiments-2026-09-11.md).
Those measurements exposed two distinct missing guarantees: completing the last
DSP job did not release every worker's graph reference, and completing a block
did not wake threads waiting for another queue item.

**The fix is implemented, and the normal release `metal_seq` binary has been
rebuilt.** On macOS, the callback now attempts 64 empty polls before waiting up
to 50 µs for new work or completion. Worker count and the workers' existing
8-spin / 50 µs policy are unchanged. Linux retains its previous callback policy.

## Final interleaved comparison

| Configuration | Runs | Process CPU | Mean callback budget | p99 callback budget |
|---|---:|---:|---:|---:|
| 4 workers, original | 2 | 135.3% | 37.7% | 39.7% |
| 4 workers, new default | 2 | 125.2% | 38.0% | 40.2% |
| 6 workers, original | 2 | 147.3% | 35.0% | 38.0% |
| 6 workers, new default | 2 | 137.7% | 35.6% | 39.3% |
| 6 workers, 200 µs candidate | 2 | 137.1% | 35.5% | 39.1% |

These are medians of two independently launched runs per configuration on the
final revision; p99 is the median of each run's p99. Four workers saved **10.1
process CPU percentage points**, with mean callback budget increasing by about
**0.3 points**. Six workers saved **9.7 CPU points**, with mean callback budget
increasing by about **0.6 points**. Tail latency increased modestly: roughly
0.5 and 1.3 callback-budget points at p99 respectively. This is a CPU saving
with a small deadline-headroom cost, not a DSP speedup.

The 200 µs candidate at six workers saved only another 0.6 CPU point in this
final comparison. The shorter 50 µs setting is the shipping choice; the data
does not justify making the longer wait the default.

Across **29 successful live runs** (three previous-failure reproductions,
16 first-revision comparisons, and ten final comparisons), all processes
completed teardown. Every measured window had zero over-budget callbacks and
zero new late/dropped events. Startup/warmup late counters are kept separately
and are not zero in every run. Earlier prototype and final-revision results
are retained separately rather than pooled into the table above.

This establishes the tradeoff on scene 4 of this project, not a universal
optimum or a guarantee for a nearly overloaded project. The UI is absent from
the harness, so compare the CPU deltas instead of its absolute percentage to
the full app. No build or C test ran during the live measurement windows.

## Ownership and notification protocol

The engine now has a session gate combining a closed bit and a worker reference
count. Publishing a block opens this gate with release ordering. Workers acquire
a reference before loading the graph pointer, and hold it through DSP,
bookkeeping, and queue waits. The session condition-variable predicate only reads
engine-owned state, so a late worker cannot inspect a reclaimed graph.

At block completion the callback closes the gate to new references, notifies
queue waiters, and waits for the existing references to leave. Only then does it
clear the graph pointer and return. This also protects consecutive blocks on the
same graph pointer and diagnostic bookkeeping after the last job counter update.
Graph reset, edits, retirement, and destruction therefore happen after worker
accesses end. Workers above the adaptive limit remain on the session condition
variable instead of polling the graph every 50 microseconds.

The ready queue has an explicit completion predicate. A waiter registers before
checking both available work and completion. Publication and registration use a
shared sequentially consistent order: either the waiter observes the condition,
or the publisher observes the registered waiter and supplies a semaphore wake.
Completion wakes all registered waiters, including a parked callback. Semaphore
credits are drained only at the exclusive block boundary. Publishing work while
there are no waiters needs no saved wake credit, because a future waiter checks
for that work before parking.

The host still stops rendering before stopping the worker pool, as the Rust
engine API already requires. After a render returns, no worker remains in that
graph's queue. Pool shutdown therefore wakes only the between-block condition
variable and joins workers; it does not depend on a queue timeout expiring.
Workgroup acknowledgement counters are initialized before their version is
published, and zero-worker pools require no acknowledgements.

## Validation scope

All **52 focused C checks passed**: 32 experiment-policy checks, four normal-build
checks, and 16 checks with diagnostic bookkeeping enabled. The matrix includes queue saturation, ordered summation,
block-event delivery, and a new completion/lifetime regression. The new test
registers six waiters with 60-second timeouts under a 15-second watchdog: all
must be released by completion. It also checks publication racing waiter
registration, late entry after completion, repeated reset, and 1,024 graph
blocks across zero, one, three, and six-worker pools. A final joining DSP node
waits for the idle workers to register, so the lifetime case does not rely on
an arbitrary sleep. Graphs are destroyed and recreated while the pool remains
alive. A disposable mutation build that deliberately omitted the session barrier
failed at the assertion that no queue waiters remain after render return. This
confirms the regression exercises the lifetime guarantee.

Six final-revision offline renders (zero workers, callback wait 50 µs at four
and six workers, callback wait 200 µs at six workers, one worker spin, and a
1,000 µs worker wait) matched the original audio bit-for-bit:
`c07dd241fba0e5016e967c4df4bf89c45284681e2c5490f69d2084cecd25a253`.
They used one warmup bar and two measured bars. Their timing fields are not
performance evidence: rendering is unpaced and the normal app build ran
concurrently with these output-only checks.

Live runs use the real CoreAudio/scheduler/DSP path with silent device output,
visible scene 4 of `phsyicsxdf`, four warmup bars and eight measured bars. UI
rendering is absent, so absolute CPU numbers are lower than the full app's
Activity Monitor reading. Baseline and candidate processes run sequentially in
shuffled order. CPU is the aggregate process percentage; callback percentages
are render time divided by the available frame budget. Callback budget misses
and event counters are useful signals, not an acoustic glitch detector.

Apple's installed AddressSanitizer runtime hung during shadow-memory setup,
and ThreadSanitizer crashed in its initialization through dyld/libSystem before
`main`. These attempts provide no sanitizer validation. Their traces are retained.
Linux execution has not been validated on this Mac.

## Evidence

Raw configs, private project snapshots, per-block metrics, binary hashes,
source snapshots, build/test logs and sanitizer startup traces are retained at:
`.local/benchmarks/audio-wait-fix-2026-09-11`.
The original baseline binary is retained there for interleaved comparisons.

The runner accepts `--binary` to select a build and `--baseline-binary` to
interleave an older build. `--baseline-names` defaults to `workers-4,workers-6`,
so previously failing wait policies are never run on the old binary implicitly.
