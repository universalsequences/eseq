#ifndef GRAPH_ENGINE_H
#define GRAPH_ENGINE_H

#include "graph_types.h"

extern _Atomic uint64_t g_param_push_count;
extern _Atomic uint64_t g_param_push_fail_count;

// ===================== Runtime Graph Types =====================

// Live graph edge buffer
typedef struct {
  float *buf;   // size = block_size
  int refcount; // number of input ports consuming this signal
  bool in_use;
  int src_node;  // who writes this edge
  int src_port;  // which output port
  int next_free; // free-list link (-1 = end of list; only valid when !in_use)
} LiveEdge;

typedef struct RTNode {
  uint64_t logical_id; // stable ID for migration/params
  char *debug_name;
  NodeVTable vtable;
  void *state;       // aligned, preallocated
  size_t state_size; // size of allocated state for watch list copying
  int nInputs, nOutputs;

  // Port-based edge management
  int32_t
      *inEdgeId; // array[nInputs]: edge ID per input port (-1 if unconnected)
  int32_t *
      outEdgeId; // array[nOutputs]: edge ID per output port (-1 if unconnected)

  // For auto-sum: for each input port on this node, store an optional SUM node
  // id
  int32_t *fanin_sum_node_id; // array[nInputs]: SUM node ID per input port (-1
                              // if none)

  // === OPTIMIZATION: Pre-cached IO pointers ===
  // These are rebuilt only on topology changes, not every block
  // Eliminates per-block input/output loops in bind_and_run_live
  float **cached_inPtrs;   // Pre-resolved input buffer pointers
  float **cached_outPtrs;  // Pre-resolved output buffer pointers
  float *cached_inInline[MAX_IO];
  float *cached_outInline[MAX_IO];
  bool io_cache_valid;     // False if topology changed, needs rebuild

  // scheduling
  int32_t *succ; // successor node indices
  int succCount; // number of nodes that depend on this node's output
} RTNode;

typedef struct {
  float *buffer;
  int size;
  int channel_count;
} BufferDesc;

// ===================== Live Editing System =====================

typedef struct RetireEntry {
  void *ptr;
  void (*deleter)(void *); // e.g., free; or custom
} RetireEntry;

typedef struct LiveGraph {
  // --- Node & edge storage ---
  RTNode *nodes;
  BufferDesc *buffers;
  int node_count, node_capacity, buffer_count, buffer_capacity;
  LiveEdge *edges;
  int edge_capacity;
  int edge_free_head; // head of free-list threading through LiveEdge.next_free
  int block_size;
  float *silence_buf;  // zero buffer for unconnected inputs
  float *scratch_null; // throwaway buffer for unconnected outputs

  // --- DAG scheduling ---
  struct {
    atomic_int *pending;   // per-node remaining-predecessor count (reset each block)
    int *indegree;         // maintained incrementally at edit time
    bool *is_orphaned;     // nodes unreachable from DAC
    ReadyQ *readyQueue;
    _Atomic int jobsInFlight;
    int32_t *source_nodes; // cached source node IDs (indegree=0, has outputs)
    int source_count;
    int source_capacity;
    int cached_total_jobs;
    bool has_cycle;
    bool dirty;            // true when topology changed, triggers cache rebuild
    bool *completed_this_block; // per-node flag: set at completion site, cleared at block start
  } sched;

  ParamRing *params;

  int dac_node_id;
  int num_channels;
  const char *label;
  GraphEditQueue *graphEditQueue;

  // --- Deferred cleanup (drained once per block) ---
  struct {
    RetireEntry *list;
    int count;
    int capacity;
  } retire;

  // --- Failed operation tracking ---
  uint64_t *failed_ids;
  int failed_ids_count;
  int failed_ids_capacity;

  _Atomic int next_node_id;
  _Atomic int next_buffer_id;
  _Atomic int edit_batch_depth;
  _Atomic uint64_t active_edit_batch_serial;
  _Atomic uint64_t next_edit_batch_serial;
  _Atomic uint64_t committed_edit_batch_serial;

  // --- Watch list & state monitoring ---
  struct {
    int *list;
    int count;
    int capacity;
    pthread_mutex_t mutex;
    void **snapshots;        // state copies indexed by node_id
    size_t *sizes;           // state sizes indexed by node_id
    pthread_rwlock_t lock;
  } watch;

} LiveGraph;

// ===================== Worker Pool / Engine =====================

typedef struct Engine {
  pthread_t *threads;
  int workerCount; // maximum worker count started by host
  _Atomic int activeWorkerLimit; // adaptive per-block worker count (0..workerCount)
  _Atomic int runFlag; // 1 = running, 0 = shutdown

  _Atomic(LiveGraph *) workSession; // published at block start, NULL after
  _Atomic int sessionFrames;        // number of frames for current block

  // Block-start wake mechanism
  pthread_mutex_t sess_mtx; // protects sess_cv wait/signal
  pthread_cond_t sess_cv;   // workers sleep here between blocks

  int sampleRate;
  int blockSize;

  // Optional: Audio Workgroup token for co-scheduling (Apple-only usage)
  // Stored as opaque pointer to avoid hard dependency in public header.
  _Atomic(void *) oswg;          // os_workgroup_t when available
  _Atomic int oswg_join_pending; // set to 1 to wake workers for workgroup join
  _Atomic int oswg_join_remaining; // count of workers that need to see the flag
  _Atomic int oswg_version;      // incremented on each workgroup change for re-join detection
  _Atomic int rt_log; // enable lightweight debug prints from workers
  _Atomic int graph_log; // enable per-block audiograph scheduler/output trace
  _Atomic int rt_time_constraint; // apply Mach RT time-constraint policy
} Engine;

void engine_start_workers(int workers);
void engine_stop_workers(void);
void apply_params(LiveGraph *g);

// Optional: supply an OS Workgroup object
// (kAudioOutputUnitProperty_OSWorkgroup) Pass the os_workgroup_t you obtained
// from the audio unit. No-ops on platforms without OS Workgroup support.
void engine_set_os_workgroup(void *oswg);
void engine_clear_os_workgroup(void);

// Enable or disable minimal worker join logging (off by default).
void engine_enable_rt_logging(int enable);
void engine_enable_graph_logging(int enable);

// Enable/disable Mach time-constraint scheduling for workers (Apple only).
void engine_enable_rt_time_constraint(int enable);

// ===================== Live Graph Operations =====================

LiveGraph *create_live_graph(int initial_capacity, int block_size,
                             const char *label, int num_channels);
void destroy_live_graph(LiveGraph *lg);
int apply_add_node(LiveGraph *lg, NodeVTable vtable, size_t state_size,
                   uint64_t logical_id, const char *name, int nInputs,
                   int nOutputs, const void *initial_state);

int live_add_oscillator(LiveGraph *lg, float freq_hz, const char *name);
int live_add_gain(LiveGraph *lg, float gain_value, const char *name);
int live_add_number(LiveGraph *lg, float value, const char *name);
int live_add_mixer2(LiveGraph *lg, const char *name);
int live_add_mixer8(LiveGraph *lg, const char *name);
int live_add_sum(LiveGraph *lg, const char *name, int nInputs);
bool apply_connect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                   int dst_port);
bool apply_disconnect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                      int dst_port);
bool apply_delete_node(LiveGraph *lg, int node_id);
void process_live_block(LiveGraph *lg, int nframes);

// ===================== Queue-based API (Pre-allocated IDs)
// =====================
int add_node(LiveGraph *lg, NodeVTable vtable, size_t state_size,
             const char *name, int nInputs, int nOutputs,
             const void *initial_state, size_t initial_state_size);
bool delete_node(LiveGraph *lg, int node_id);
bool graph_connect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                   int dst_port);
void begin_graph_edit_batch(LiveGraph *lg);
void end_graph_edit_batch(LiveGraph *lg);
bool graph_disconnect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                      int dst_port);

bool hot_swap_node(LiveGraph *lg, int node_id, NodeVTable vt, size_t state_size,
                   int nin, int nout, bool xfade,
                   void (*migrate)(void *, void *), const void *initial_state,
                   size_t initial_state_size);

bool replace_keep_edges(LiveGraph *lg, int node_id, NodeVTable vt,
                        size_t state_size, int nin, int nout, bool xfade,
                        void (*migrate)(void *, void *),
                        const void *initial_state, size_t initial_state_size);

bool is_failed_node(LiveGraph *lg, int node_id);
void add_failed_id(LiveGraph *lg, uint64_t logical_id);
int find_live_output(LiveGraph *lg);

// ===================== Live Engine Operations =====================

void process_next_block(LiveGraph *lg, float *output_buffer, int nframes);
void retire_later(LiveGraph *lg, void *ptr, void (*deleter)(void *));

// ===================== Watch List API =====================

bool add_node_to_watchlist(LiveGraph *lg, int node_id);
bool remove_node_from_watchlist(LiveGraph *lg, int node_id);
void *get_node_state(LiveGraph *lg, int node_id, size_t *state_size);
bool get_node_state_into(LiveGraph *lg, int node_id, void *out,
                         size_t out_capacity, size_t *state_size);

void update_orphaned_status(LiveGraph *lg);

// ===================== Buffer API =====================

// Create a buffer with optional initial data. Pass NULL for source_data to create empty buffer.
// Returns buffer_id on success, -1 on failure.
int create_buffer(LiveGraph *lg, int size, int channel_count,
                  const float *source_data);

// Hot-swap buffer contents. Copies source_data into existing buffer.
// Returns true on success, false if buffer doesn't exist or on failure.
int hot_swap_buffer(LiveGraph *lg, int buffer_id, const float *source_data,
                    int size, int channel_count);

// ===================== VTable Creation Functions =====================

// ===================== Global Engine Instance =====================

extern Engine g_engine;
void initialize_engine(int block_size, int sample_rate);

// Allocate port arrays (inEdgeId, outEdgeId, fanin_sum_node_id) if not yet allocated.
bool ensure_port_arrays(RTNode *n);

// Allocate (or reuse from pool) an edge buffer; returns edge id or -1.
int alloc_edge(LiveGraph *lg);

#endif // GRAPH_ENGINE_H
