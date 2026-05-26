#include "graph_api.h"
#include "graph_edit.h"
#include "graph_nodes.h"

static bool using_inline_in_cache(const RTNode *node) {
  return node->cached_inPtrs == (float **)node->cached_inInline;
}

static bool using_inline_out_cache(const RTNode *node) {
  return node->cached_outPtrs == (float **)node->cached_outInline;
}

LiveGraph *create_live_graph(int initial_capacity, int block_size,
                             const char *label, int num_channels) {
  LiveGraph *lg = calloc(1, sizeof(LiveGraph));

  // Node storage
  lg->node_capacity = initial_capacity;
  lg->nodes = calloc(lg->node_capacity, sizeof(RTNode));
  lg->sched.pending = calloc(lg->node_capacity, sizeof(atomic_int));
  lg->sched.indegree = calloc(lg->node_capacity, sizeof(int));
  lg->sched.is_orphaned = calloc(lg->node_capacity, sizeof(bool));
  lg->sched.completed_this_block = calloc(lg->node_capacity, sizeof(bool));

  // Buffer storage
  lg->buffer_capacity = initial_capacity;
  lg->buffers = calloc(lg->buffer_capacity, sizeof(BufferDesc));
  lg->buffer_count = 0;

  // Edge pool (start with generous capacity)
  lg->edge_capacity = initial_capacity * 32;
  lg->block_size = block_size;
  lg->num_channels = (num_channels > 0) ? num_channels : 1; // Default to mono

  // Initialize Retire List
  lg->retire.capacity = 32;
  lg->retire.count = 0;
  lg->retire.list = calloc(lg->retire.capacity, sizeof(RetireEntry));

  // Edge pool with free-list (buffers allocated lazily in alloc_edge)
  lg->edges = calloc(lg->edge_capacity, sizeof(LiveEdge));
  for (int i = 0; i < lg->edge_capacity; i++) {
    lg->edges[i].buf = NULL;
    lg->edges[i].in_use = false;
    lg->edges[i].refcount = 0;
    lg->edges[i].src_node = -1;
    lg->edges[i].src_port = -1;
    lg->edges[i].next_free = i + 1; // chain into free list
  }
  lg->edges[lg->edge_capacity - 1].next_free = -1; // terminate list
  lg->edge_free_head = 0;

  // Support buffers for port system
  lg->silence_buf = alloc_aligned(64, block_size * sizeof(float));
  lg->scratch_null = alloc_aligned(64, block_size * sizeof(float));
  memset(lg->silence_buf, 0,
         block_size * sizeof(float)); // keep silence buffer zeroed

  // Ready queue (ReadyQ with MPMC + semaphore for thread safety)
  // BURST FIX: Increased capacity from 1024 to 4096 for wide graphs
  lg->sched.readyQueue = rq_create(4096);
  if (!lg->sched.readyQueue) {
    // Handle allocation failure - edge buffers are NULL (lazy allocation)
    free(lg->edges);
    free(lg->silence_buf);
    free(lg->scratch_null);
    free(lg->nodes);
    free(lg->sched.pending);
    free(lg->sched.indegree);
    free(lg->sched.is_orphaned);
    free(lg->sched.completed_this_block);
    free(lg);
    return NULL;
  }

  // Parameter mailbox
  lg->params = calloc(1, sizeof(ParamRing));

  lg->graphEditQueue = calloc(1, sizeof(GraphEditQueue));
  geq_init(lg->graphEditQueue, 8192 * 16);

  // Initialize failed IDs tracking
  lg->failed_ids_capacity = 64; // Start with reasonable capacity
  lg->failed_ids = calloc(lg->failed_ids_capacity, sizeof(uint64_t));
  lg->failed_ids_count = 0;

  // Initialize atomic node ID counter (start at 1 to avoid confusion with DAC
  // at 0)
  atomic_init(&lg->next_node_id, 1);
  atomic_init(&lg->next_buffer_id, 0);
  atomic_init(&lg->edit_batch_depth, 0);
  atomic_init(&lg->active_edit_batch_serial, 0);
  atomic_init(&lg->next_edit_batch_serial, 1);
  atomic_init(&lg->committed_edit_batch_serial, 0);

  // Initialize watch list system
  lg->watch.capacity = 16;
  lg->watch.list = calloc(lg->watch.capacity, sizeof(int));
  lg->watch.count = 0;
  pthread_mutex_init(&lg->watch.mutex, NULL);

  // Initialize state store arrays (indexed by node_id)
  lg->watch.snapshots = calloc(lg->node_capacity, sizeof(void *));
  lg->watch.sizes = calloc(lg->node_capacity, sizeof(size_t));
  pthread_rwlock_init(&lg->watch.lock, NULL);

  // Initialize scheduling cache (optimization to avoid O(n) per block)
  lg->sched.source_capacity = 256;  // Start with reasonable capacity
  lg->sched.source_nodes = calloc(lg->sched.source_capacity, sizeof(int32_t));
  lg->sched.source_count = 0;
  lg->sched.cached_total_jobs = 0;
  lg->sched.has_cycle = false;
  lg->sched.dirty = true;  // Force initial rebuild

  // Automatically create the DAC node at index 0
  // DAC has one input and one output per channel
  int dac_id = apply_add_node(lg, DAC_VTABLE, 0, 0, "DAC", lg->num_channels,
                              lg->num_channels, NULL);
  if (dac_id >= 0) {
    lg->dac_node_id = dac_id; // Remember the DAC node

    RTNode *dac = &lg->nodes[dac_id];
    dac->nInputs = lg->num_channels; // DAC has N inputs (one per channel)
    dac->nOutputs =
        lg->num_channels; // DAC has N outputs (for reading final audio)
    if (!ensure_port_arrays(dac)) {
      // DAC node allocation failed - this is critical
      destroy_live_graph(lg);
      return NULL;
    }

    // Allocate output edges for each channel of the DAC
    for (int ch = 0; ch < lg->num_channels; ch++) {
      int output_edge = alloc_edge(lg);
      if (output_edge >= 0) {
        dac->outEdgeId[ch] = output_edge; // Use port-based system
      }
    }
  }

  lg->label = label;
  return lg;
}

void destroy_live_graph(LiveGraph *lg) {
  if (!lg)
    return;

  // Free all edge buffers
  if (lg->edges) {
    for (int i = 0; i < lg->edge_capacity; i++) {
      if (lg->edges[i].buf) {
        free(lg->edges[i].buf);
      }
    }
    free(lg->edges);
  }

  // Free all node state and port arrays
  if (lg->nodes) {
    for (int i = 0; i < lg->node_count; i++) { // Use count, not capacity
      RTNode *node = &lg->nodes[i];

      if (node->state) {
        free(node->state);
      }
      if (node->debug_name) {
        free(node->debug_name);
      }
      if (node->inEdgeId) {
        free(node->inEdgeId);
      }
      if (node->outEdgeId) {
        free(node->outEdgeId);
      }
      if (node->fanin_sum_node_id) {
        free(node->fanin_sum_node_id);
      }
      if (node->succ) {
        free(node->succ);
      }
      // Free cached IO pointers
      if (node->cached_inPtrs) {
        if (!using_inline_in_cache(node)) {
          free(node->cached_inPtrs);
        }
      }
      if (node->cached_outPtrs) {
        if (!using_inline_out_cache(node)) {
          free(node->cached_outPtrs);
        }
      }
    }
    free(lg->nodes);
  }

  // Free scheduling arrays
  if (lg->sched.pending)
    free(lg->sched.pending);
  if (lg->sched.indegree)
    free(lg->sched.indegree);
  if (lg->sched.is_orphaned)
    free(lg->sched.is_orphaned);
  if (lg->sched.completed_this_block)
    free(lg->sched.completed_this_block);

  // Free support buffers
  if (lg->silence_buf)
    free(lg->silence_buf);
  if (lg->scratch_null)
    free(lg->scratch_null);

  // Free queues
  if (lg->sched.readyQueue)
    rq_destroy(lg->sched.readyQueue);
  if (lg->params)
    free(lg->params);
  if (lg->graphEditQueue) {
    geq_deinit(lg->graphEditQueue);
    free(lg->graphEditQueue);
  }

  // Free failed IDs tracking
  if (lg->failed_ids)
    free(lg->failed_ids);

  // Free watch list system
  if (lg->watch.list)
    free(lg->watch.list);
  pthread_mutex_destroy(&lg->watch.mutex);

  // Free state snapshots
  if (lg->watch.snapshots) {
    for (int i = 0; i < lg->node_capacity; i++) {
      if (lg->watch.snapshots[i]) {
        free(lg->watch.snapshots[i]);
      }
    }
    free(lg->watch.snapshots);
  }
  if (lg->watch.sizes)
    free(lg->watch.sizes);
  pthread_rwlock_destroy(&lg->watch.lock);

  // Free retire list
  if (lg->retire.list)
    free(lg->retire.list);

  // Free scheduling cache
  if (lg->sched.source_nodes)
    free(lg->sched.source_nodes);

  // Free the graph itself
  free(lg);
}

static int allocate_logical_id(LiveGraph *lg) {
  return atomic_fetch_add(&lg->next_node_id, 1);
}

static int finalize_live_add(LiveGraph *lg, int node_id, NodeVTable vtable,
                             size_t state_size, const char *name, int nInputs,
                             int nOutputs, const void *initial_state) {
  int result = apply_add_node(lg, vtable, state_size, (uint64_t)node_id, name,
                              nInputs, nOutputs, initial_state);
  if (result < 0) {
    add_failed_id(lg, node_id);
    return -1;
  }
  return result;
}

int live_add_oscillator(LiveGraph *lg, float freq_hz, const char *name) {
  if (!lg)
    return -1;
  int node_id = allocate_logical_id(lg);
  float init_state[1];
  if (freq_hz < 0.0f)
    freq_hz = 0.0f;
  float sr = g_engine.sampleRate > 0 ? (float)g_engine.sampleRate : 48000.0f;
  init_state[0] = freq_hz / sr;
  return finalize_live_add(lg, node_id, OSC_VTABLE,
                           OSC_MEMORY_SIZE * sizeof(float), name, 0, 1,
                           init_state);
}

int live_add_gain(LiveGraph *lg, float gain_value, const char *name) {
  if (!lg)
    return -1;
  int node_id = allocate_logical_id(lg);
  float init_state[1];
  init_state[0] = gain_value;
  return finalize_live_add(lg, node_id, GAIN_VTABLE,
                           GAIN_MEMORY_SIZE * sizeof(float), name, 2, 1,
                           init_state);
}

int live_add_number(LiveGraph *lg, float value, const char *name) {
  if (!lg)
    return -1;
  int node_id = allocate_logical_id(lg);
  float init_state[1];
  init_state[0] = value;
  return finalize_live_add(lg, node_id, NUMBER_VTABLE,
                           NUMBER_MEMORY_SIZE * sizeof(float), name, 0, 1,
                           init_state);
}

int live_add_mixer2(LiveGraph *lg, const char *name) {
  if (!lg)
    return -1;
  int node_id = allocate_logical_id(lg);
  return finalize_live_add(lg, node_id, MIX2_VTABLE, 0, name, 2, 1, NULL);
}

int live_add_mixer8(LiveGraph *lg, const char *name) {
  if (!lg)
    return -1;
  int node_id = allocate_logical_id(lg);
  return finalize_live_add(lg, node_id, MIX8_VTABLE, 0, name, 8, 1, NULL);
}

int live_add_sum(LiveGraph *lg, const char *name, int nInputs) {
  if (!lg || nInputs <= 0)
    return -1;
  int node_id = allocate_logical_id(lg);
  return finalize_live_add(lg, node_id, SUM_VTABLE, 0, name, nInputs, 1, NULL);
}

static inline uint64_t current_batch_serial(LiveGraph *lg) {
  return atomic_load_explicit(&lg->active_edit_batch_serial, memory_order_acquire);
}

int add_node(LiveGraph *lg, NodeVTable vtable, size_t state_size,
             const char *name, int nInputs, int nOutputs,
             const void *initial_state, size_t initial_state_size) {
  // Atomically allocate the next node ID (which is also the array index)
  int node_id = atomic_fetch_add(&lg->next_node_id, 1);

  // Create a copy of initial_state if provided
  void *initial_state_copy = NULL;
  char *name_copy = NULL;
  if (initial_state && initial_state_size > 0) {
    initial_state_copy = malloc(initial_state_size);
    if (!initial_state_copy) {
      add_failed_id(lg, node_id);
      return -1;
    }
    memcpy(initial_state_copy, initial_state, initial_state_size);
  }
  if (name) {
    name_copy = strdup(name);
    if (!name_copy) {
      if (initial_state_copy) {
        free(initial_state_copy);
      }
      add_failed_id(lg, node_id);
      return -1;
    }
  }

  // Create the command
  GraphEditCmd cmd = {
      .op = GE_ADD_NODE,
      .batch_serial = current_batch_serial(lg),
      .u.add_node = {
          .vt = vtable,
          .state_size = state_size,
          .logical_id =
              node_id, // Use node_id as the logical_id (they're the same)
          .name = name_copy,
          .nInputs = nInputs,
          .nOutputs = nOutputs,
          .initial_state = initial_state_copy,
          .initial_state_size = initial_state_size}};

  // Queue the command
  if (!geq_push(lg->graphEditQueue, &cmd)) {
    // Queue full - consider this a failure
    if (initial_state_copy) {
      free(initial_state_copy);
    }
    if (name_copy) {
      free(name_copy);
    }
    add_failed_id(lg, node_id);
    return -1;
  }

  // Return the pre-allocated node ID (which is both logical_id and array index)
  return node_id;
}

int add_gain_node(LiveGraph *lg, float gain_value, const char *name) {
  if (!lg)
    return -1;

  float init_state[1];
  init_state[0] = gain_value;
  return add_node(lg, GAIN_VTABLE, GAIN_MEMORY_SIZE * sizeof(float), name, 2,
                  1, init_state, sizeof(init_state));
}

void begin_graph_edit_batch(LiveGraph *lg) {
  if (!lg)
    return;
  int old_depth = atomic_fetch_add_explicit(&lg->edit_batch_depth, 1, memory_order_acq_rel);
  if (old_depth == 0) {
    uint64_t serial =
        atomic_fetch_add_explicit(&lg->next_edit_batch_serial, 1, memory_order_acq_rel);
    atomic_store_explicit(&lg->active_edit_batch_serial, serial, memory_order_release);
  }
}

void end_graph_edit_batch(LiveGraph *lg) {
  if (!lg)
    return;
  int old_depth =
      atomic_fetch_sub_explicit(&lg->edit_batch_depth, 1, memory_order_acq_rel);
  if (old_depth == 1) {
    uint64_t serial =
        atomic_load_explicit(&lg->active_edit_batch_serial, memory_order_acquire);
    atomic_store_explicit(&lg->committed_edit_batch_serial, serial, memory_order_release);
    atomic_store_explicit(&lg->active_edit_batch_serial, 0, memory_order_release);
  } else if (old_depth <= 0) {
    atomic_store_explicit(&lg->edit_batch_depth, 0, memory_order_release);
    atomic_store_explicit(&lg->active_edit_batch_serial, 0, memory_order_release);
  }
}

int create_buffer(LiveGraph *lg, int size, int channel_count,
                  const float *source_data) {
  int buffer_id = atomic_fetch_add(&lg->next_buffer_id, 1);

  // Copy source data if provided (so caller can free their copy)
  float *source_copy = NULL;
  size_t source_size = 0;
  if (source_data && size > 0 && channel_count > 0) {
    source_size = (size_t)size * (size_t)channel_count * sizeof(float);
    source_copy = malloc(source_size);
    if (source_copy) {
      memcpy(source_copy, source_data, source_size);
    }
  }

  GraphEditCmd cmd = {.op = GE_CREATE_BUFFER,
                      .batch_serial = current_batch_serial(lg),
                      .u.create_buffer = {.buffer_id = buffer_id,
                                          .size = size,
                                          .channel_count = channel_count,
                                          .source_data = source_copy,
                                          .source_data_size = source_size}};

  if (!geq_push(lg->graphEditQueue, &cmd)) {
    if (source_copy)
      free(source_copy);
    return -1;
  }
  return buffer_id;
}

int hot_swap_buffer(LiveGraph *lg, int buffer_id, const float *source_data,
                    int size, int channel_count) {
  if (!lg || buffer_id < 0 || !source_data || size <= 0 || channel_count <= 0) {
    return false;
  }

  // Copy source data (so caller can free their copy)
  size_t source_size = (size_t)size * (size_t)channel_count * sizeof(float);
  float *source_copy = malloc(source_size);
  if (!source_copy) {
    return false;
  }
  memcpy(source_copy, source_data, source_size);

  GraphEditCmd cmd = {.op = GE_HOTSWAP_BUFFER,
                      .batch_serial = current_batch_serial(lg),
                      .u.hotswap_buffer = {.buffer_id = buffer_id,
                                           .size = size,
                                           .channel_count = channel_count,
                                           .source_data = source_copy,
                                           .source_data_size = source_size}};

  if (!geq_push(lg->graphEditQueue, &cmd)) {
    free(source_copy);
    return false;
  }
  return true;
}

bool is_failed_node(LiveGraph *lg, int logical_id) {
  // Check if this logical ID is in the failed list
  for (int i = 0; i < lg->failed_ids_count; i++) {
    if (lg->failed_ids[i] == (uint64_t)logical_id) {
      return true;
    }
  }
  return false;
}

// ===================== Queue-based API =====================

bool delete_node(LiveGraph *lg, int node_id) {
  GraphEditCmd cmd = {.op = GE_REMOVE_NODE,
                      .batch_serial = current_batch_serial(lg),
                      .u.remove_node = {.node_id = node_id}};

  return geq_push(lg->graphEditQueue, &cmd);
}

bool graph_connect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                   int dst_port) {
  // Check if either node has failed
  if (is_failed_node(lg, src_node) || is_failed_node(lg, dst_node)) {
    return false;
  }

  GraphEditCmd cmd = {.op = GE_CONNECT,
                      .batch_serial = current_batch_serial(lg),
                      .u.connect = {.src_id = src_node,
                                    .src_port = src_port,
                                    .dst_id = dst_node,
                                    .dst_port = dst_port}};

  return geq_push(lg->graphEditQueue, &cmd);
}

bool graph_disconnect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                      int dst_port) {
  GraphEditCmd cmd = {.op = GE_DISCONNECT,
                      .batch_serial = current_batch_serial(lg),
                      .u.disconnect = {.src_id = src_node,
                                       .src_port = src_port,
                                       .dst_id = dst_node,
                                       .dst_port = dst_port}};

  return geq_push(lg->graphEditQueue, &cmd);
}

bool hot_swap_node(LiveGraph *lg, int node_id, NodeVTable vt, size_t state_size,
                   int nin, int nout, bool xfade,
                   void (*migrate)(void *, void *), const void *initial_state,
                   size_t initial_state_size) {
  (void)xfade;
  (void)migrate;
  if (is_failed_node(lg, node_id)) {
    return false;
  }

  // Create a copy of initial_state if provided
  void *initial_state_copy = NULL;
  if (initial_state && initial_state_size > 0) {
    initial_state_copy = malloc(initial_state_size);
    if (initial_state_copy) {
      memcpy(initial_state_copy, initial_state, initial_state_size);
    }
  }

  GraphEditCmd cmd = {.op = GE_HOT_SWAP_NODE,
                      .batch_serial = current_batch_serial(lg),
                      .u.hot_swap_node =
                          {
                              .vt = vt,
                              .state_size = state_size,
                              .node_id = node_id,
                              .new_nInputs = nin,
                              .new_nOutputs = nout,
                              .initial_state = initial_state_copy,
                              .initial_state_size = initial_state_size,
                          }

  };
  if (!geq_push(lg->graphEditQueue, &cmd)) {
    // Queue push failed - clean up copied memory
    if (initial_state_copy) {
      free(initial_state_copy);
    }
    return false;
  }
  return true;
}

bool replace_keep_edges(LiveGraph *lg, int node_id, NodeVTable vt,
                        size_t state_size, int nin, int nout, bool xfade,
                        void (*migrate)(void *, void *),
                        const void *initial_state, size_t initial_state_size) {
  (void)xfade;
  (void)migrate;
  // Create a copy of initial_state if provided
  void *initial_state_copy = NULL;
  if (initial_state && initial_state_size > 0) {
    initial_state_copy = malloc(initial_state_size);
    if (initial_state_copy) {
      memcpy(initial_state_copy, initial_state, initial_state_size);
    }
  }

  GraphEditCmd cmd = {.op = GE_REPLACE_KEEP_EDGES,
                      .batch_serial = current_batch_serial(lg),
                      .u.replace_keep_edges = {
                          .vt = vt,
                          .state_size = state_size,
                          .node_id = node_id,
                          .new_nInputs = nin,
                          .new_nOutputs = nout,
                          .initial_state = initial_state_copy,
                          .initial_state_size = initial_state_size,

                      }};
  if (!geq_push(lg->graphEditQueue, &cmd)) {
    // Queue push failed - clean up copied memory
    if (initial_state_copy) {
      free(initial_state_copy);
    }
    return false;
  }
  return true;
}

void retire_later(LiveGraph *lg, void *ptr, void (*deleter)(void *)) {
  if (!ptr)
    return;
  if (lg->retire.count >= lg->retire.capacity) {
    int new_cap = lg->retire.capacity ? lg->retire.capacity * 2 : 16;
    RetireEntry *new_list =
        realloc(lg->retire.list, new_cap * sizeof(RetireEntry));
    if (!new_list)
      return; // allocation failed, skip retirement
    lg->retire.list = new_list;
    lg->retire.capacity = new_cap;
  }
  lg->retire.list[lg->retire.count++] =
      (RetireEntry){.ptr = ptr, .deleter = deleter};
}

// ===================== Watch List Implementation =====================

bool add_node_to_watchlist(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0)
    return false;
  GraphEditCmd cmd = {.op = GE_ADD_WATCH,
                      .batch_serial = current_batch_serial(lg),
                      .u.add_watch = {.node_id = node_id}};
  return geq_push(lg->graphEditQueue, &cmd);
}

bool remove_node_from_watchlist(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0)
    return false;
  GraphEditCmd cmd = {.op = GE_REMOVE_WATCH,
                      .batch_serial = current_batch_serial(lg),
                      .u.remove_watch = {.node_id = node_id}};
  return geq_push(lg->graphEditQueue, &cmd);
}

void *get_node_state(LiveGraph *lg, int node_id, size_t *state_size) {
  if (!lg || node_id < 0 || node_id >= lg->node_capacity) {
    if (state_size)
      *state_size = 0;
    return NULL;
  }

  pthread_rwlock_rdlock(&lg->watch.lock);

  void *result = NULL;
  size_t size = 0;

  if (lg->watch.snapshots[node_id] && lg->watch.sizes[node_id] > 0) {
    size = lg->watch.sizes[node_id];
    result = malloc(size);
    if (result) {
      memcpy(result, lg->watch.snapshots[node_id], size);
    }
  }

  pthread_rwlock_unlock(&lg->watch.lock);

  if (state_size)
    *state_size = size;
  return result;
}

bool get_node_state_into(LiveGraph *lg, int node_id, void *out,
                         size_t out_capacity, size_t *state_size) {
  if (state_size)
    *state_size = 0;
  if (!lg || !out || node_id < 0 || node_id >= lg->node_capacity) {
    return false;
  }

  pthread_rwlock_rdlock(&lg->watch.lock);

  bool copied = false;
  if (lg->watch.snapshots[node_id] && lg->watch.sizes[node_id] > 0) {
    size_t size = lg->watch.sizes[node_id];
    if (state_size)
      *state_size = size;
    if (size <= out_capacity) {
      memcpy(out, lg->watch.snapshots[node_id], size);
      copied = true;
    }
  }

  pthread_rwlock_unlock(&lg->watch.lock);
  return copied;
}

// Debug: dump graph topology to stderr
void debug_dump_graph(LiveGraph *lg) {
  if (!lg) return;
  fprintf(stderr, "\n=== GRAPH DUMP (node_count=%d) ===\n", lg->node_count);
  fprintf(stderr, "  scratch_null=%p\n", (void*)lg->scratch_null);
  for (int i = 0; i < lg->node_count; i++) {
    RTNode *n = &lg->nodes[i];
    if (n->vtable.process == NULL && n->nInputs == 0 && n->nOutputs == 0)
      continue;
    fprintf(stderr, "  Node %d: nIn=%d nOut=%d orphaned=%d cache_valid=%d\n",
            i, n->nInputs, n->nOutputs,
            lg->sched.is_orphaned[i], n->io_cache_valid);
    if (n->inEdgeId) {
      for (int p = 0; p < n->nInputs; p++) {
        int eid = n->inEdgeId[p];
        if (eid >= 0) {
          fprintf(stderr, "    in[%d] edge=%d src_node=%d src_port=%d buf=%p\n",
                  p, eid, lg->edges[eid].src_node, lg->edges[eid].src_port,
                  (void*)lg->edges[eid].buf);
        } else {
          fprintf(stderr, "    in[%d] UNCONNECTED\n", p);
        }
      }
    }
    if (n->outEdgeId) {
      for (int p = 0; p < n->nOutputs; p++) {
        int eid = n->outEdgeId[p];
        if (eid >= 0) {
          fprintf(stderr, "    out[%d] edge=%d buf=%p\n",
                  p, eid, (void*)lg->edges[eid].buf);
        } else {
          fprintf(stderr, "    out[%d] UNCONNECTED\n", p);
        }
      }
    }
    if (n->cached_outPtrs) {
      for (int p = 0; p < n->nOutputs; p++) {
        fprintf(stderr, "    cached_out[%d]=%p%s\n",
                p, (void*)n->cached_outPtrs[p],
                n->cached_outPtrs[p] == lg->scratch_null ? " (SCRATCH_NULL!)" : "");
      }
    }
  }
  fprintf(stderr, "=== END DUMP ===\n\n");
}
