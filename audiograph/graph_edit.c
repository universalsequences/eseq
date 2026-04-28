#include "graph_edit.h"
#include "graph_nodes.h"
#include "hot_swap.h"
#include <assert.h>

#define EDGE_BUFFER_PAD_FLOATS 64

static bool using_inline_in_cache(const RTNode *node) {
  return node->cached_inPtrs == (float **)node->cached_inInline;
}

static bool using_inline_out_cache(const RTNode *node) {
  return node->cached_outPtrs == (float **)node->cached_outInline;
}

// ===================== Port & Edge Allocation =====================

bool ensure_port_arrays(RTNode *n) {
  if (!n || n->nInputs < 0 || n->nOutputs < 0)
    return false;

  // Reject unreasonable values to guard against corruption
  if (n->nInputs > 1000 || n->nOutputs > 1000)
    return false;

  if (!n->inEdgeId && n->nInputs > 0) {
    n->inEdgeId = (int32_t *)malloc(sizeof(int32_t) * n->nInputs);
    if (!n->inEdgeId)
      return false;
    for (int i = 0; i < n->nInputs; i++)
      n->inEdgeId[i] = -1;
  }

  if (!n->outEdgeId && n->nOutputs > 0) {
    n->outEdgeId = (int32_t *)malloc(sizeof(int32_t) * n->nOutputs);
    if (!n->outEdgeId)
      return false;
    for (int i = 0; i < n->nOutputs; i++)
      n->outEdgeId[i] = -1;
  }

  if (!n->fanin_sum_node_id && n->nInputs > 0) {
    n->fanin_sum_node_id = (int32_t *)malloc(sizeof(int32_t) * n->nInputs);
    if (!n->fanin_sum_node_id)
      return false;
    for (int i = 0; i < n->nInputs; i++)
      n->fanin_sum_node_id[i] = -1;
  }

  return true;
}

int alloc_edge(LiveGraph *lg) {
  int i = lg->edge_free_head;
  if (i < 0)
    return -1; // pool exhausted

  lg->edge_free_head = lg->edges[i].next_free;
  lg->edges[i].in_use = true;
  lg->edges[i].refcount = 0;
  lg->edges[i].src_node = -1;
  lg->edges[i].src_port = -1;
  lg->edges[i].next_free = -1;
  if (!lg->edges[i].buf) {
    lg->edges[i].buf = alloc_aligned(
        64, (lg->block_size + EDGE_BUFFER_PAD_FLOATS) * sizeof(float));
  }
  memset(lg->edges[i].buf, 0,
         sizeof(float) * (lg->block_size + EDGE_BUFFER_PAD_FLOATS));
  return i;
}

// ===================== Edge Retirement & Helpers =====================

static void retire_edge(LiveGraph *lg, int eid) {
  if (eid < 0 || eid >= lg->edge_capacity)
    return;
  if (lg->edges[eid].buf) {
    memset(lg->edges[eid].buf, 0,
           sizeof(float) * (lg->block_size + EDGE_BUFFER_PAD_FLOATS));
    free(lg->edges[eid].buf);
    lg->edges[eid].buf = NULL;
  }
  lg->edges[eid].refcount = 0;
  lg->edges[eid].in_use = false;
  lg->edges[eid].src_node = -1;
  lg->edges[eid].src_port = -1;
  // Push onto free list
  lg->edges[eid].next_free = lg->edge_free_head;
  lg->edge_free_head = eid;
}

static bool still_connected_S_to_D(LiveGraph *lg, int S_id, int D_id) {
  RTNode *S = &lg->nodes[S_id];
  RTNode *D = &lg->nodes[D_id];
  if (!S->outEdgeId || !D->inEdgeId)
    return false;
  for (int so = 0; so < S->nOutputs; so++) {
    int eid = S->outEdgeId[so];
    if (eid < 0)
      continue;
    for (int di = 0; di < D->nInputs; di++) {
      if (D->inEdgeId[di] == eid)
        return true;
    }
  }
  return false;
}

// Helper function to validate if a node is in a valid state
static bool is_node_valid(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0 || node_id >= lg->node_count) {
    return false;
  }
  RTNode *node = &lg->nodes[node_id];

  // Check for reasonable port counts (prevent corruption-induced huge values)
  if (node->nInputs < 0 || node->nInputs > 1000 || node->nOutputs < 0 ||
      node->nOutputs > 1000) {
    return false;
  }

  // If node has ports, it should have arrays (or NULL if never allocated)
  if (node->nInputs > 0 && node->inEdgeId == NULL) {
    // This is acceptable - arrays get allocated on demand
  }
  if (node->nOutputs > 0 && node->outEdgeId == NULL) {
    // This is acceptable - arrays get allocated on demand
  }

  return true;
}

// Helper function to validate if a SUM node is still active and safe to use
static bool is_sum_node_valid(LiveGraph *lg, int sum_id) {
  if (!is_node_valid(lg, sum_id)) {
    return false;
  }
  RTNode *node = &lg->nodes[sum_id];
  // A deleted SUM node will have NULL inEdgeId and outEdgeId arrays
  // AND zero port counts
  return (node->inEdgeId != NULL && node->outEdgeId != NULL &&
          node->nInputs > 0 && node->nOutputs > 0);
}

static void remove_successor(RTNode *src, int succ_id) {
  for (int i = 0; i < src->succCount; i++) {
    if (src->succ[i] == succ_id) {
      // swap-with-last
      int last = src->succCount - 1;
      if (i != last)
        src->succ[i] = src->succ[last];
      src->succCount--;
      if (src->succCount == 0) {
        free(src->succ);
        src->succ = NULL;
      } else {
        src->succ =
            (int32_t *)realloc(src->succ, sizeof(int32_t) * src->succCount);
      }
      return;
    }
  }
}

// Add successor (swap-with-last removal used on disconnect)
static inline void add_successor_port(RTNode *src, int succ_id) {
  src->succ =
      (int32_t *)realloc(src->succ, sizeof(int32_t) * (src->succCount + 1));
  src->succ[src->succCount++] = succ_id;
}

// Optional: check if successor already present (prevent dup edges in succ list)
static inline bool has_successor(const RTNode *src, int succ_id) {
  for (int i = 0; i < src->succCount; i++)
    if (src->succ[i] == succ_id)
      return true;
  return false;
}

// Helper: increment indegree only on first connection between src→dst
static inline void indegree_inc_on_first_pred(LiveGraph *lg, int src, int dst) {
  if (!has_successor(&lg->nodes[src], dst)) {
    add_successor_port(&lg->nodes[src], dst);
    lg->sched.indegree[dst]++; // count unique predecessor once
  }
}

// Helper: decrement indegree only on last disconnection between src→dst
static inline void indegree_dec_on_last_pred(LiveGraph *lg, int src, int dst) {
  // For hot swap scenarios, we need to check if this specific edge being
  // disconnected was the last connection, not just if connections still exist
  // after this disconnect operation completes
  if (!still_connected_S_to_D(lg, src, dst)) {
    if (lg->sched.indegree[dst] > 0)
      lg->sched.indegree[dst]--;
    remove_successor(&lg->nodes[src], dst);
  }
}

// Forward declarations
bool apply_delete_node_internal(LiveGraph *lg, int node_id);
static bool apply_add_watchlist(LiveGraph *lg, int node_id);
static bool apply_remove_watchlist(LiveGraph *lg, int node_id);
static bool grow_node_capacity(LiveGraph *lg, int required_capacity);

// Internal version that skips update_orphaned_status for batched operations
static bool apply_add_watchlist_internal(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0 || node_id >= lg->node_capacity) {
    return false;
  }

  pthread_mutex_lock(&lg->watch.mutex);

  for (int i = 0; i < lg->watch.count; i++) {
    if (lg->watch.list[i] == node_id) {
      pthread_mutex_unlock(&lg->watch.mutex);
      return true;
    }
  }

  if (lg->watch.count >= lg->watch.capacity) {
    int new_cap = lg->watch.capacity ? lg->watch.capacity * 2 : 16;
    int *new_list = realloc(lg->watch.list, new_cap * sizeof(int));
    if (!new_list) {
      pthread_mutex_unlock(&lg->watch.mutex);
      return false;
    }
    lg->watch.list = new_list;
    lg->watch.capacity = new_cap;
  }

  lg->watch.list[lg->watch.count++] = node_id;
  pthread_mutex_unlock(&lg->watch.mutex);

  return true;
}

static bool apply_add_watchlist(LiveGraph *lg, int node_id) {
  bool result = apply_add_watchlist_internal(lg, node_id);
  if (result) {
    update_orphaned_status(lg);
  }
  return result;
}

// Internal version that skips update_orphaned_status for batched operations
static bool apply_remove_watchlist_internal(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0) {
    return false;
  }

  pthread_mutex_lock(&lg->watch.mutex);

  for (int i = 0; i < lg->watch.count; i++) {
    if (lg->watch.list[i] == node_id) {
      for (int j = i; j < lg->watch.count - 1; j++) {
        lg->watch.list[j] = lg->watch.list[j + 1];
      }
      lg->watch.count--;

      pthread_rwlock_wrlock(&lg->watch.lock);
      if (node_id < lg->node_capacity && lg->watch.snapshots[node_id]) {
        free(lg->watch.snapshots[node_id]);
        lg->watch.snapshots[node_id] = NULL;
        lg->watch.sizes[node_id] = 0;
      }
      pthread_rwlock_unlock(&lg->watch.lock);

      pthread_mutex_unlock(&lg->watch.mutex);
      return true;
    }
  }

  pthread_mutex_unlock(&lg->watch.mutex);
  return false;
}

static bool apply_remove_watchlist(LiveGraph *lg, int node_id) {
  bool result = apply_remove_watchlist_internal(lg, node_id);
  if (result) {
    update_orphaned_status(lg);
  }
  return result;
}

// Recursive function to mark nodes reachable from DAC (port-based only)
static void mark_reachable_from_dac(LiveGraph *lg, int node_id, bool *visited) {
  if (node_id < 0 || node_id >= lg->node_count || visited[node_id])
    return;
  visited[node_id] = true;
  lg->sched.is_orphaned[node_id] = false;

  RTNode *node = &lg->nodes[node_id];
  if (!node->inEdgeId)
    return;

  for (int i = 0; i < node->nInputs; i++) {
    int eid = node->inEdgeId[i];
    if (eid < 0)
      continue;
    int src = lg->edges[eid].src_node;
    if (src >= 0)
      mark_reachable_from_dac(lg, src, visited);
  }
}

// Update orphaned status for all nodes based on DAC reachability
void update_orphaned_status(LiveGraph *lg) {
  // Mark scheduling cache as dirty - topology has changed
  lg->sched.dirty = true;

  // Invalidate IO caches for all nodes - topology has changed
  for (int i = 0; i < lg->node_count; i++) {
    lg->nodes[i].io_cache_valid = false;
  }

  // First, mark all nodes as orphaned (except watched nodes)
  for (int i = 0; i < lg->node_count; i++) {
    // Check if this node is in the watchlist
    bool is_watched = false;
    pthread_mutex_lock(&lg->watch.mutex);
    for (int w = 0; w < lg->watch.count; w++) {
      if (lg->watch.list[w] == i) {
        is_watched = true;
        break;
      }
    }
    pthread_mutex_unlock(&lg->watch.mutex);
    // Watched nodes are never orphaned - they always stay active
    lg->sched.is_orphaned[i] = !is_watched;
  }

  // NEW: Also treat all upstream dependencies of watched nodes as non-orphaned
  // This allows analyzer/scope-like nodes (and their inputs) to stay active
  // even when not connected to the DAC.
  int watch_count = 0;
  int *watch_nodes = NULL;
  pthread_mutex_lock(&lg->watch.mutex);
  if (lg->watch.count > 0) {
    watch_count = lg->watch.count;
    watch_nodes = (int *)malloc(sizeof(int) * watch_count);
    if (watch_nodes) {
      memcpy(watch_nodes, lg->watch.list, sizeof(int) * watch_count);
    } else {
      // Allocation failed; fall back to using 0 count
      watch_count = 0;
    }
  }
  pthread_mutex_unlock(&lg->watch.mutex);

  if (watch_count > 0 && watch_nodes) {
    bool *wvisited = calloc(lg->node_count, sizeof(bool));
    if (wvisited) {
      for (int wi = 0; wi < watch_count; wi++) {
        int nid = watch_nodes[wi];
        if (nid >= 0 && nid < lg->node_count) {
          // DFS upstream from each watched node
          mark_reachable_from_dac(lg, nid, wvisited);
        }
      }
      free(wvisited);
    }
    free(watch_nodes);
  }

  // If no DAC node exists, non-watched nodes remain orphaned. Upstream of
  // watched nodes has already been handled above.
  if (lg->dac_node_id < 0) {
    return;
  }

  // Check if DAC has any inputs first - if not, leave it orphaned (unless
  // watched)
  RTNode *dac = &lg->nodes[lg->dac_node_id];
  bool dac_has_inputs = false;
  if (dac->inEdgeId) {
    for (int i = 0; i < dac->nInputs; i++) {
      if (dac->inEdgeId[i] >= 0) {
        dac_has_inputs = true;
        break;
      }
    }
  }

  if (!dac_has_inputs) {
    // DAC has no inputs, non-watched nodes remain orphaned (upstream of
    // watched nodes already marked above)
    return;
  }
  // DAC has inputs, marking as connected
  // Use DFS to mark all nodes reachable from DAC
  bool *visited = calloc(lg->node_count, sizeof(bool));
  mark_reachable_from_dac(lg, lg->dac_node_id, visited);
  free(visited);

  // NOTE: Indegree is maintained incrementally by connect/disconnect operations
  // via indegree_inc_on_first_pred() and indegree_dec_on_last_pred().
  // The O(n²) recomputation that was here was removed for performance -
  // with 7000 nodes it caused ~50ms audio dropouts on topology changes.
}

// ===================== Failed ID Tracking =====================

void add_failed_id(LiveGraph *lg, uint64_t logical_id) {
  // Expand capacity if needed
  if (lg->failed_ids_count >= lg->failed_ids_capacity) {
    lg->failed_ids_capacity *= 2;
    lg->failed_ids =
        realloc(lg->failed_ids, lg->failed_ids_capacity * sizeof(uint64_t));
  }
  lg->failed_ids[lg->failed_ids_count++] = logical_id;
}

bool grow_buffer_capacity(LiveGraph *lg) {
  int old_capacity = lg->buffer_capacity;
  lg->buffer_capacity *= 2;
  BufferDesc *new_buffers = calloc(lg->buffer_capacity, sizeof(BufferDesc));
  if (!new_buffers) {
    return false;
  }
  for (int i = 0; i < old_capacity && i < lg->buffer_count; i++) {
    new_buffers[i].buffer =
        lg->buffers[i].buffer; // copy pointer of buffer over
    new_buffers[i].size = lg->buffers[i].size;
    new_buffers[i].channel_count = lg->buffers[i].channel_count;
  }
  free(lg->buffers);
  lg->buffers = new_buffers;
  return true;
}

bool apply_create_buffer(LiveGraph *lg, int buffer_id, int size,
                         int channel_count, const float *source_data,
                         size_t source_data_size) {
  if (buffer_id < 0) {
    return false;
  }
  if (buffer_id >= lg->buffer_capacity) {
    if (!grow_buffer_capacity(lg)) {
      // failed to grow buffer capacity
      // TODO - mark buffer as invalid
      return false;
    }
  }

  BufferDesc *buf = &lg->buffers[buffer_id];
  memset(buf, 0, sizeof(BufferDesc));

  buf->size = size;
  buf->channel_count = channel_count;
  size_t total_samples = (size_t)channel_count * (size_t)size;
  buf->buffer = (float *)calloc(total_samples, sizeof(float));

  if (!buf->buffer) {
    return false;
  }

  // Copy source data if provided
  if (source_data && source_data_size > 0) {
    size_t bytes_to_copy = total_samples * sizeof(float);
    if (source_data_size < bytes_to_copy) {
      bytes_to_copy = source_data_size; // Don't overflow source
    }
    memcpy(buf->buffer, source_data, bytes_to_copy);
  }

  lg->buffer_count++;
  return true;
}

bool apply_hotswap_buffer(LiveGraph *lg, int buffer_id, int new_size,
                          int new_channel_count, const float *source_data,
                          size_t source_data_size) {
  if (buffer_id < 0 || buffer_id >= lg->buffer_capacity) {
    return false;
  }

  BufferDesc *buf = &lg->buffers[buffer_id];
  if (!buf->buffer) {
    // Buffer doesn't exist yet - can't hotswap
    return false;
  }

  size_t new_total_samples = (size_t)new_channel_count * (size_t)new_size;
  size_t old_total_samples = (size_t)buf->channel_count * (size_t)buf->size;

  // Reallocate if size changed
  if (new_total_samples != old_total_samples) {
    float *new_buffer = (float *)calloc(new_total_samples, sizeof(float));
    if (!new_buffer) {
      return false;
    }
    free(buf->buffer);
    buf->buffer = new_buffer;
    buf->size = new_size;
    buf->channel_count = new_channel_count;
  }

  // Copy new data into buffer
  size_t bytes_to_copy = new_total_samples * sizeof(float);
  if (source_data_size < bytes_to_copy) {
    bytes_to_copy = source_data_size; // Don't overflow source
  }
  memcpy(buf->buffer, source_data, bytes_to_copy);

  return true;
}

// to be called from block-boundary (i.e. before each block is executed)
bool apply_graph_edits(GraphEditQueue *r, LiveGraph *lg) {
  GraphEditCmd cmd;
  GraphEditCmd next_cmd;

  bool all_ok = true;
  bool needs_orphan_update = false;
  int reserved_node_id =
      atomic_load_explicit(&lg->next_node_id, memory_order_acquire) - 1;

  if (reserved_node_id >= lg->node_capacity &&
      !grow_node_capacity(lg, reserved_node_id)) {
    return false;
  }

  while (true) {
    uint64_t committed_batch_serial = atomic_load_explicit(
        &lg->committed_edit_batch_serial, memory_order_acquire);
    if (!geq_peek(r, &next_cmd)) {
      break;
    }
    if (next_cmd.batch_serial != 0 &&
        next_cmd.batch_serial > committed_batch_serial) {
      break;
    }
    if (!geq_pop(r, &cmd)) {
      break;
    }
    bool ok = true;
    bool topology_changed = false;

    // then we have a cmd to run
    // Use internal versions that skip update_orphaned_status for batching
    switch (cmd.op) {
    case GE_ADD_NODE: {
      int nid = apply_add_node(lg, cmd.u.add_node.vt, cmd.u.add_node.state_size,
                               cmd.u.add_node.logical_id, cmd.u.add_node.name,
                               cmd.u.add_node.nInputs, cmd.u.add_node.nOutputs,
                               cmd.u.add_node.initial_state);
      ok = nid >= 0;
      topology_changed = ok;
      if (!ok) {
        // Track the failed logical ID
        add_failed_id(lg, cmd.u.add_node.logical_id);
      }
      // Clean up allocated initial_state memory
      if (cmd.u.add_node.initial_state) {
        free(cmd.u.add_node.initial_state);
      }
      if (cmd.u.add_node.name) {
        free(cmd.u.add_node.name);
      }
      break;
    }
    case GE_REMOVE_NODE:
      ok = apply_delete_node_internal(lg, cmd.u.remove_node.node_id);
      topology_changed = ok;
      break;
    case GE_CONNECT: {
      ok = apply_connect_internal(lg, cmd.u.connect.src_id,
                                  cmd.u.connect.src_port, cmd.u.connect.dst_id,
                                  cmd.u.connect.dst_port);
      topology_changed = ok;
      break;
    }
    case GE_DISCONNECT: {
      ok = apply_disconnect_internal(
          lg, cmd.u.disconnect.src_id, cmd.u.disconnect.src_port,
          cmd.u.disconnect.dst_id, cmd.u.disconnect.dst_port);
      topology_changed = ok;
      break;
    }
    case GE_HOT_SWAP_NODE:
      ok = apply_hot_swap(lg, &cmd.u.hot_swap_node);
      break;
    case GE_REPLACE_KEEP_EDGES:
      ok = apply_replace_keep_edges_internal(lg, &cmd.u.replace_keep_edges);
      topology_changed = ok;
      break;
    case GE_ADD_WATCH:
      ok = apply_add_watchlist_internal(lg, cmd.u.add_watch.node_id);
      topology_changed = ok;
      break;
    case GE_REMOVE_WATCH:
      ok = apply_remove_watchlist_internal(lg, cmd.u.remove_watch.node_id);
      topology_changed = ok;
      break;
    case GE_CREATE_BUFFER:
      ok = apply_create_buffer(
          lg, cmd.u.create_buffer.buffer_id, cmd.u.create_buffer.size,
          cmd.u.create_buffer.channel_count, cmd.u.create_buffer.source_data,
          cmd.u.create_buffer.source_data_size);
      // Free the copied source data after apply
      if (cmd.u.create_buffer.source_data) {
        free(cmd.u.create_buffer.source_data);
      }
      break;
    case GE_HOTSWAP_BUFFER:
      ok = apply_hotswap_buffer(lg, cmd.u.hotswap_buffer.buffer_id,
                                cmd.u.hotswap_buffer.size,
                                cmd.u.hotswap_buffer.channel_count,
                                cmd.u.hotswap_buffer.source_data,
                                cmd.u.hotswap_buffer.source_data_size);
      // Free the copied source data after apply
      if (cmd.u.hotswap_buffer.source_data) {
        free(cmd.u.hotswap_buffer.source_data);
      }
      break;
    default: {
      ok = false; // unknown op
      break;
    }
    }
    if (!ok) {
      all_ok = false;
    }
    if (topology_changed) {
      needs_orphan_update = true;
    }
  }

  // Batch the orphan status update
  if (needs_orphan_update) {
    update_orphaned_status(lg);
  }

  return all_ok;
}

/**
 * Internal disconnect - skips update_orphaned_status for batched operations.
 * Also uses apply_delete_node_internal to avoid nested orphan updates.
 */
bool apply_disconnect_internal(LiveGraph *lg, int src_node, int src_port,
                               int dst_node, int dst_port) {
  if (!lg || src_node < 0 || src_node >= lg->node_count || dst_node < 0 ||
      dst_node >= lg->node_count) {
    return false;
  }

  RTNode *S = &lg->nodes[src_node];
  RTNode *D = &lg->nodes[dst_node];

  if (src_port < 0 || src_port >= S->nOutputs || dst_port < 0 ||
      dst_port >= D->nInputs) {
    return false;
  }

  if (!D->inEdgeId || !S->outEdgeId)
    return false;

  // Check if dst_port has a SUM node
  int sum_id = D->fanin_sum_node_id ? D->fanin_sum_node_id[dst_port] : -1;

  if (sum_id == -1) {
    // No SUM node - handle as direct connection
    int eid_in = D->inEdgeId[dst_port];
    int eid_out = S->outEdgeId[src_port];

    // Nothing connected on that dst port → nothing to do
    if (eid_in < 0)
      return false;

    // Ensure we're disconnecting the intended link
    if (eid_out < 0 || eid_in != eid_out) {
      return false;
    }

    // Unwire the destination port
    D->inEdgeId[dst_port] = -1;

    // Update successor list and indegree on the source node
    if (!still_connected_S_to_D(lg, src_node, dst_node)) {
      lg->sched.indegree[dst_node]--; // last S→D connection gone
      remove_successor(S, dst_node);
    }
    if (lg->sched.indegree[dst_node] < 0)
      lg->sched.indegree[dst_node] = 0;

    // Edge refcount and retirement if last consumer
    LiveEdge *e = &lg->edges[eid_in];
    if (e->refcount > 0)
      e->refcount--;
    if (e->refcount == 0) {
      retire_edge(lg, eid_in);
      S->outEdgeId[src_port] = -1;
    }
  } else {
    // SUM node exists - find which SUM input corresponds to src_node:src_port
    if (!is_sum_node_valid(lg, sum_id)) {
      // SUM node was deleted - treat as successful disconnect
      return true;
    }
    RTNode *SUM = &lg->nodes[sum_id];
    int src_eid = S->outEdgeId[src_port];
    if (src_eid < 0)
      return false; // Source not connected

    // Find the SUM input that matches this source
    int sum_input_idx = -1;
    for (int i = 0; i < SUM->nInputs; i++) {
      if (SUM->inEdgeId[i] == src_eid) {
        sum_input_idx = i;
        break;
      }
    }

    if (sum_input_idx == -1)
      return false; // Source not connected to this SUM

    // Disconnect src_node from SUM
    SUM->inEdgeId[sum_input_idx] = -1;
    indegree_dec_on_last_pred(lg, src_node,
                              sum_id); // ✅ only if last edge S→SUM

    // Handle edge refcount
    LiveEdge *e = &lg->edges[src_eid];
    if (e->refcount > 0)
      e->refcount--;
    if (e->refcount == 0) {
      retire_edge(lg, src_eid);
      S->outEdgeId[src_port] = -1;
    }

    // Compact SUM inputs (remove the gap)
    for (int i = sum_input_idx; i < SUM->nInputs - 1; i++) {
      SUM->inEdgeId[i] = SUM->inEdgeId[i + 1];
    }
    SUM->nInputs--;

    // Validate SUM node before realloc - it may have been deleted
    if (!SUM->inEdgeId) {
      // SUM node was deleted - skip realloc
      return true;
    }

    if (SUM->nInputs > 0) {
      int32_t *new_ptr = realloc(SUM->inEdgeId, SUM->nInputs * sizeof(int32_t));
      if (new_ptr) {
        SUM->inEdgeId = new_ptr;
      }
      // If realloc fails, keep using the old pointer - it's still valid just
      // larger
    }

    // FIX: After shrinking, update remaining source nodes to point to their own
    // edges instead of the SUM output (restore direct fan-out capability)
    if (SUM->nInputs >= 2) {
      for (int i = 0; i < SUM->nInputs; i++) {
        int eid = SUM->inEdgeId[i];
        if (eid >= 0 && eid < lg->edge_capacity && lg->edges[eid].in_use) {
          int src_node = lg->edges[eid].src_node;
          int src_port = lg->edges[eid].src_port;
          // SAFETY: Don't update the SUM node itself, only regular source nodes
          if (src_node >= 0 && src_node < lg->node_capacity &&
              src_node != sum_id && lg->nodes[src_node].outEdgeId) {
            // Update source to point to its own edge (not SUM output)
            lg->nodes[src_node].outEdgeId[src_port] = eid;
          }
        }
      }
    }

    // Handle SUM collapse cases
    if (SUM->nInputs == 0) {
      // No inputs left - remove SUM and clear destination
      D->inEdgeId[dst_port] = -1;
      D->fanin_sum_node_id[dst_port] = -1;
      indegree_dec_on_last_pred(lg, sum_id, dst_node);

      // Delete the SUM node (use internal to avoid nested orphan updates)
      apply_delete_node_internal(lg, sum_id);
    } else if (SUM->nInputs == 1) {
      // Only one input left - collapse SUM back to direct connection.
      // Deletion of a source feeding a SUM can leave a cleared (-1) or retired
      // edge in the SUM input array until a later logical disconnect compacts
      // the SUM. Do not index lg->edges unless the remaining SUM input still
      // names a live edge.
      int remaining_eid = SUM->inEdgeId[0];
      int sum_out = SUM->outEdgeId ? SUM->outEdgeId[0] : -1;
      if (remaining_eid < 0 || remaining_eid >= lg->edge_capacity ||
          !lg->edges[remaining_eid].in_use) {
        D->inEdgeId[dst_port] = -1;
        D->fanin_sum_node_id[dst_port] = -1;
        indegree_dec_on_last_pred(lg, sum_id, dst_node);
        apply_delete_node_internal(lg, sum_id);
        return true;
      }

      // Find the source of the remaining edge
      int remaining_src = lg->edges[remaining_eid].src_node;
      int remaining_src_port = lg->edges[remaining_eid].src_port;
      if (remaining_src < 0 || remaining_src >= lg->node_count ||
          remaining_src_port < 0) {
        D->inEdgeId[dst_port] = -1;
        D->fanin_sum_node_id[dst_port] = -1;
        indegree_dec_on_last_pred(lg, sum_id, dst_node);
        apply_delete_node_internal(lg, sum_id);
        return true;
      }

      // Create a new edge for the direct connection
      int direct_eid = alloc_edge(lg);
      if (direct_eid < 0)
        return false;

      // Set up the new direct edge
      lg->edges[direct_eid].src_node = remaining_src;
      lg->edges[direct_eid].src_port = remaining_src_port;
      lg->edges[direct_eid].refcount = 1; // Start with destination consumption

      // Ensure buffer is allocated and valid
      if (!lg->edges[direct_eid].buf) {
        lg->edges[direct_eid].buf =
            alloc_aligned(64, (lg->block_size + EDGE_BUFFER_PAD_FLOATS) *
                                  sizeof(float));
        if (!lg->edges[direct_eid].buf) {
          retire_edge(lg, direct_eid);
          return false;
        }
        memset(lg->edges[direct_eid].buf, 0,
               sizeof(float) * (lg->block_size + EDGE_BUFFER_PAD_FLOATS));
      }

      // Connect destination to new edge
      D->inEdgeId[dst_port] = direct_eid;
      D->fanin_sum_node_id[dst_port] = -1;

      // Wire source to new edge and update successor relationships
      bool added = false;
      if (remaining_src >= 0) {
        // Check if source already has an outgoing edge
        int existing_out_eid =
            lg->nodes[remaining_src].outEdgeId[remaining_src_port];
        if (existing_out_eid >= 0) {
          // Source already has an outgoing edge
          retire_edge(lg, direct_eid);

          // Use the existing shared edge
          direct_eid = existing_out_eid;
          D->inEdgeId[dst_port] = direct_eid;

          // Increment refcount for the new consumer
          lg->edges[direct_eid].refcount++;
        } else {
          // No existing edge - use our new one
          lg->nodes[remaining_src].outEdgeId[remaining_src_port] = direct_eid;
        }

        if (!has_successor(&lg->nodes[remaining_src], dst_node)) {
          add_successor_port(&lg->nodes[remaining_src], dst_node);
          added = true;
        } else {
          // remaining_src was ALREADY a predecessor of dst_node (via another port).
          // SUM contributed 1 to dst_node's indegree, but remaining_src taking over
          // doesn't add a new unique predecessor. So we must decrement indegree
          // to account for SUM being removed.
          if (lg->sched.indegree[dst_node] > 0) {
            lg->sched.indegree[dst_node]--;
          }
        }
        // Remove SUM from source's successors
        remove_successor(&lg->nodes[remaining_src], sum_id);
      }

      // Update all other consumers of the old SUM output edge to point to
      // the new direct edge before deleting SUM
      if (sum_out >= 0) {
        for (int consumer_node = 0; consumer_node < lg->node_count;
             consumer_node++) {
          if (consumer_node == dst_node)
            continue; // Already updated above
          RTNode *consumer = &lg->nodes[consumer_node];
          if (!consumer->inEdgeId || consumer->nInputs <= 0)
            continue;

          for (int consumer_port = 0; consumer_port < consumer->nInputs;
               consumer_port++) {
            if (consumer->inEdgeId[consumer_port] == sum_out) {
              // This consumer was using the old SUM output - update to direct
              // edge
              consumer->inEdgeId[consumer_port] = direct_eid;
              lg->edges[direct_eid].refcount++; // New consumer
            }
          }
        }
      }

      // Let apply_delete_node_internal handle all edge cleanup and retirement
      apply_delete_node_internal(lg, sum_id);

      // Restore the direct connection that apply_delete_node cleared
      D->inEdgeId[dst_port] = direct_eid;

      // apply_delete_node corrupted the source node's output
      // structure We need to restore both the outEdgeId pointer and ensure
      // nOutputs is correct
      if (remaining_src >= 0 && remaining_src < lg->node_count) {
        RTNode *src_node = &lg->nodes[remaining_src];

        // If apply_delete_node corrupted the output structure, restore it
        if (src_node->nOutputs == 0 || src_node->outEdgeId == NULL) {
          // The source should have at least 1 output (the one we're creating)
          if (src_node->nOutputs == 0) {
            src_node->nOutputs = 1; // Restore to original output count
          }
          if (src_node->outEdgeId == NULL) {
            // Reallocate the output edge array
            src_node->outEdgeId = calloc(src_node->nOutputs, sizeof(int32_t));
            for (int i = 0; i < src_node->nOutputs; i++) {
              src_node->outEdgeId[i] = -1; // Initialize to -1
            }
          }
        }

        // Now set the direct edge connection
        if (remaining_src_port >= 0 &&
            remaining_src_port < src_node->nOutputs) {
          src_node->outEdgeId[remaining_src_port] = direct_eid;
        }
      }

      // No indegree increment needed - apply_delete_node already handled the
      // SUM→dst disconnection and the new direct connection replaces it with
      // the same logical indegree
    }
    // If SUM->nInputs > 1, SUM continues to exist with fewer inputs
  }

  return true;
}

/**
 * Disconnect a logical connection between src_node:src_port and
 * dst_node:dst_port. This function is transparent to SUM nodes - it handles the
 * hidden SUM logic automatically. Returns true if the logical connection
 * existed and was removed.
 */
bool apply_disconnect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                      int dst_port) {
  bool result =
      apply_disconnect_internal(lg, src_node, src_port, dst_node, dst_port);
  if (result) {
    update_orphaned_status(lg);
  }
  return result;
}

bool apply_delete_node_internal(LiveGraph *lg, int node_id) {
  if (!lg || node_id < 0 || node_id >= lg->node_count) {
    return false;
  }

  RTNode *node = &lg->nodes[node_id];

  // Remove from watchlist before freeing node state. Otherwise deleted watched
  // nodes leave stale IDs/snapshots behind, which can accumulate in dynamic
  // patches/presets and keep adding watchlist scan/snapshot overhead.
  apply_remove_watchlist_internal(lg, node_id);

  // Special handling for DAC node
  if (lg->dac_node_id == node_id) {
    lg->dac_node_id = -1; // Clear DAC reference
  }

  // 1) Disconnect all inbound connections (clean up other nodes' outputs to
  // this node) - FIX: Use edge's src info and proper refcount handling
  if (node->inEdgeId && node->nInputs > 0) {
    for (int dst_port = 0; dst_port < node->nInputs; dst_port++) {
      int eid = node->inEdgeId[dst_port];
      if (eid < 0 || eid >= lg->edge_capacity || !lg->edges)
        continue;

      int s = lg->edges[eid].src_node;
      int sp = lg->edges[eid].src_port;

      // Validate source node and port
      if (s < 0 || s >= lg->node_count || sp < 0)
        continue;

      // remove this node's consumption
      node->inEdgeId[dst_port] = -1;
      // Keep successor removal coupled to the "last remaining predecessor"
      // check. Removing it unconditionally can drop a valid fanout edge while
      // leaving indegree intact when multiple source ports still feed the same
      // destination node.
      indegree_dec_on_last_pred(lg, s, node_id);

      LiveEdge *e = &lg->edges[eid];
      if (e->refcount > 0)
        e->refcount--;
      if (e->refcount == 0) {
        // now it's truly unused: retire & clear the source port
        retire_edge(lg, eid);
        RTNode *src_node = &lg->nodes[s];
        if (src_node->outEdgeId && sp < src_node->nOutputs &&
            src_node->outEdgeId[sp] == eid) {
          src_node->outEdgeId[sp] = -1;
        }
      }

      // If this inbound edge belonged to a hidden SUM node, compact/collapse is
      // handled by logical disconnects. During node deletion, at least scrub
      // the SUM bookkeeping so a later disconnect cannot treat this destination
      // port as still SUM-backed after the SUM has been deleted.
      if (node->fanin_sum_node_id && node->fanin_sum_node_id[dst_port] >= 0) {
        int sum_id = node->fanin_sum_node_id[dst_port];
        node->fanin_sum_node_id[dst_port] = -1;
        if (sum_id >= 0 && sum_id < lg->node_count) {
          RTNode *sum = &lg->nodes[sum_id];
          if (sum->inEdgeId) {
            for (int i = 0; i < sum->nInputs; i++) {
              sum->inEdgeId[i] = -1;
            }
          }
        }
      }
    }
  }

  // 2) Disconnect all outbound connections (clean up other nodes' inputs from
  // this node)
  if (node->outEdgeId && node->nOutputs > 0) {
    // Track which destinations we've touched to avoid multi-decrement per dst
    bool *touched = calloc(lg->node_count, sizeof(bool));

    for (int src_port = 0; src_port < node->nOutputs; src_port++) {
      int edge_id = node->outEdgeId[src_port];
      if (edge_id < 0)
        continue; // Port not connected

      int cleared_consumers = 0;

      // Find and clear destination nodes' input ports that consume this output
      for (int dst_node = 0; dst_node < lg->node_count; dst_node++) {
        if (dst_node == node_id)
          continue; // Skip self
        RTNode *dst = &lg->nodes[dst_node];
        if (!dst->inEdgeId || dst->nInputs <= 0)
          continue;

        for (int dst_port = 0; dst_port < dst->nInputs; dst_port++) {
          if (dst->inEdgeId[dst_port] == edge_id) {
            // Clear the destination's input port
            dst->inEdgeId[dst_port] = -1;
            cleared_consumers++;
            // Mark this destination as touched (don't decrement yet)
            touched[dst_node] = true;
          }
        }
      }

      // If this output fed a hidden SUM node, clear matching SUM inputs too.
      // Otherwise a later disconnect can compact the SUM to one input containing
      // -1/stale edge ID and crash when collapsing it back to a direct edge.
      for (int maybe_sum = 0; maybe_sum < lg->node_count; maybe_sum++) {
        if (maybe_sum == node_id)
          continue;
        RTNode *sum = &lg->nodes[maybe_sum];
        if (!sum->inEdgeId || sum->nInputs <= 0)
          continue;
        for (int sum_port = 0; sum_port < sum->nInputs; sum_port++) {
          if (sum->inEdgeId[sum_port] == edge_id) {
            sum->inEdgeId[sum_port] = -1;
            cleared_consumers++;
            touched[maybe_sum] = true;
          }
        }
      }

      // Decrease edge refcount once per cleared consumer and retire if zero
      LiveEdge *e = &lg->edges[edge_id];
      while (cleared_consumers > 0 && e->refcount > 0) {
        e->refcount--;
        cleared_consumers--;
      }
      if (e->refcount == 0) {
        retire_edge(lg, edge_id);
      }
      node->outEdgeId[src_port] = -1; // Clear this node's output
    }

    // Second pass: now that all edges are cleared, decrement indegree for
    // touched destinations
    for (int dst_node = 0; dst_node < lg->node_count; dst_node++) {
      if (touched[dst_node]) {
        indegree_dec_on_last_pred(lg, node_id, dst_node);
      }
    }

    free(touched);
  }

  // 3) Free node's memory
  if (node->state) {
    free(node->state);
    node->state = NULL;
  }
  if (node->debug_name) {
    free(node->debug_name);
    node->debug_name = NULL;
  }

  // Free port arrays
  if (node->inEdgeId) {
    free(node->inEdgeId);
    node->inEdgeId = NULL;
  }
  if (node->outEdgeId) {
    free(node->outEdgeId);
    node->outEdgeId = NULL;
  }
  if (node->fanin_sum_node_id) {
    free(node->fanin_sum_node_id);
    node->fanin_sum_node_id = NULL;
  }

  // Reset port counts to prevent reuse with invalid state
  node->nInputs = 0;
  node->nOutputs = 0;

  // Clear vtable to make invalid access more obvious
  node->vtable.process = NULL;
  node->vtable.init = NULL;
  if (node->succ) {
    free(node->succ);
    node->succ = NULL;
  }

  // Free cached IO pointers
  if (node->cached_inPtrs) {
    if (!using_inline_in_cache(node)) {
      free(node->cached_inPtrs);
    }
    node->cached_inPtrs = NULL;
  }
  if (node->cached_outPtrs) {
    if (!using_inline_out_cache(node)) {
      free(node->cached_outPtrs);
    }
    node->cached_outPtrs = NULL;
  }
  node->io_cache_valid = false;

  // 4) Clear node data and mark as deleted
  // Note: Don't clear logical_id since it's now the array index
  node->state = NULL; // Mark as deleted (state is freed above)
  memset(&node->vtable, 0, sizeof(NodeVTable)); // Clear vtable
  node->nInputs = 0;
  node->nOutputs = 0;

  // Note: We don't compact the node array to maintain stable node IDs
  // The slot can be reused by apply_add_node if needed

  return true;
}

// Ensure node's port arrays exist/are sized (call at node creation in practice)
// Grow node arrays when capacity is exceeded
static bool grow_node_capacity(LiveGraph *lg, int required_capacity) {

  if (required_capacity < lg->node_capacity) {
    return true; // Already sufficient
  }

  int new_capacity = lg->node_capacity * 2;
  while (new_capacity <= required_capacity) {
    new_capacity *= 2; // Double until sufficient
  }

  int old_capacity = lg->node_capacity;
  int new_source_capacity = lg->sched.source_capacity;
  while (new_source_capacity < new_capacity) {
    new_source_capacity *= 2;
  }

  // Allocate new arrays (don't use realloc to avoid partial corruption)
  RTNode *new_nodes = malloc(new_capacity * sizeof(RTNode));
  int *new_indegree = malloc(new_capacity * sizeof(int));
  bool *new_orphaned = malloc(new_capacity * sizeof(bool));
  atomic_int *new_pending = calloc(new_capacity, sizeof(atomic_int));
  bool *new_completed = calloc(new_capacity, sizeof(bool));
  void **new_state_snapshots = calloc(new_capacity, sizeof(void *));
  size_t *new_state_sizes = calloc(new_capacity, sizeof(size_t));
  int32_t *new_source_nodes = malloc(new_source_capacity * sizeof(int32_t));

  if (!new_nodes || !new_pending || !new_completed || !new_indegree || !new_orphaned ||
      !new_state_snapshots || !new_state_sizes || !new_source_nodes) {
    // Clean up any successful allocations
    if (new_nodes)
      free(new_nodes);
    if (new_indegree)
      free(new_indegree);
    if (new_orphaned)
      free(new_orphaned);
    if (new_pending)
      free(new_pending);
    if (new_completed)
      free(new_completed);
    if (new_state_snapshots)
      free(new_state_snapshots);
    if (new_state_sizes)
      free(new_state_sizes);
    if (new_source_nodes)
      free(new_source_nodes);
    return false;
  }

  // Copy existing data EXCEPT for dynamically allocated port arrays
  for (int i = 0; i < old_capacity; i++) {

    // Copy the basic node structure
    new_nodes[i] = lg->nodes[i];

    // But clear the port array pointers - we'll reallocate them
    new_nodes[i].inEdgeId = NULL;
    new_nodes[i].outEdgeId = NULL;
    new_nodes[i].fanin_sum_node_id = NULL;
    new_nodes[i].succ = NULL;
    new_nodes[i].cached_inPtrs = NULL;
    new_nodes[i].cached_outPtrs = NULL;
    new_nodes[i].io_cache_valid = false;  // Force rebuild after capacity growth

    // Re-allocate port arrays if the old node had them
    RTNode *old_node = &lg->nodes[i];
    RTNode *new_node = &new_nodes[i];

    if (old_node->nInputs > 0 && old_node->inEdgeId) {
      new_node->inEdgeId = malloc(old_node->nInputs * sizeof(int32_t));
      if (!new_node->inEdgeId)
        goto fail;
      memcpy(new_node->inEdgeId, old_node->inEdgeId,
             old_node->nInputs * sizeof(int32_t));
    }

    if (old_node->nOutputs > 0 && old_node->outEdgeId) {
      new_node->outEdgeId = malloc(old_node->nOutputs * sizeof(int32_t));
      if (!new_node->outEdgeId)
        goto fail;
      memcpy(new_node->outEdgeId, old_node->outEdgeId,
             old_node->nOutputs * sizeof(int32_t));
    }

    if (old_node->nInputs > 0 && old_node->fanin_sum_node_id) {
      new_node->fanin_sum_node_id = malloc(old_node->nInputs * sizeof(int32_t));
      if (!new_node->fanin_sum_node_id)
        goto fail;
      memcpy(new_node->fanin_sum_node_id, old_node->fanin_sum_node_id,
             old_node->nInputs * sizeof(int32_t));
    }

    if (old_node->succCount > 0 && old_node->succ) {
      new_node->succ = malloc(old_node->succCount * sizeof(int32_t));
      if (!new_node->succ)
        goto fail;
      memcpy(new_node->succ, old_node->succ,
             old_node->succCount * sizeof(int32_t));
    }
  }

  memcpy(new_indegree, lg->sched.indegree, old_capacity * sizeof(int));
  memcpy(new_orphaned, lg->sched.is_orphaned, old_capacity * sizeof(bool));
  if (lg->watch.snapshots)
    memcpy(new_state_snapshots, lg->watch.snapshots,
           old_capacity * sizeof(void *));
  if (lg->watch.sizes)
    memcpy(new_state_sizes, lg->watch.sizes, old_capacity * sizeof(size_t));
  if (lg->sched.source_nodes && lg->sched.source_count > 0) {
    memcpy(new_source_nodes, lg->sched.source_nodes,
           lg->sched.source_count * sizeof(int32_t));
  }

  // Copy existing atomic values
  for (int i = 0; i < old_capacity; i++) {
    int old_val = atomic_load_explicit(&lg->sched.pending[i], memory_order_relaxed);
    atomic_init(&new_pending[i], old_val);
  }

  // Zero new slots
  memset(&new_nodes[old_capacity], 0,
         (new_capacity - old_capacity) * sizeof(RTNode));
  memset(&new_indegree[old_capacity], 0,
         (new_capacity - old_capacity) * sizeof(int));
  memset(&new_orphaned[old_capacity], 0,
         (new_capacity - old_capacity) * sizeof(bool));

  // Initialize new pending slots to -1 (orphaned)
  for (int i = old_capacity; i < new_capacity; i++) {
    atomic_init(&new_pending[i], -1);
  }

  // Now properly free the old arrays including their port arrays
  for (int i = 0; i < old_capacity; i++) {
    RTNode *old_node = &lg->nodes[i];
    if (old_node->inEdgeId)
      free(old_node->inEdgeId);
    if (old_node->outEdgeId)
      free(old_node->outEdgeId);
    if (old_node->fanin_sum_node_id)
      free(old_node->fanin_sum_node_id);
    if (old_node->succ)
      free(old_node->succ);
    if (old_node->cached_inPtrs && !using_inline_in_cache(old_node))
      free(old_node->cached_inPtrs);
    if (old_node->cached_outPtrs && !using_inline_out_cache(old_node))
      free(old_node->cached_outPtrs);
  }
  free(lg->nodes);
  free(lg->sched.pending);
  free(lg->sched.indegree);
  free(lg->sched.is_orphaned);
  free(lg->sched.completed_this_block);
  if (lg->watch.snapshots)
    free(lg->watch.snapshots);
  if (lg->watch.sizes)
    free(lg->watch.sizes);

  // Update pointers and capacity
  lg->nodes = new_nodes;
  lg->sched.pending = new_pending;
  lg->sched.completed_this_block = new_completed;
  lg->sched.indegree = new_indegree;
  lg->sched.is_orphaned = new_orphaned;
  lg->watch.snapshots = new_state_snapshots;
  lg->watch.sizes = new_state_sizes;
  free(lg->sched.source_nodes);
  lg->sched.source_nodes = new_source_nodes;
  lg->sched.source_capacity = new_source_capacity;
  lg->node_capacity = new_capacity;
  lg->sched.dirty = true;

  return true;

fail:
  for (int i = 0; i < old_capacity; i++) {
    free(new_nodes[i].inEdgeId);
    free(new_nodes[i].outEdgeId);
    free(new_nodes[i].fanin_sum_node_id);
    free(new_nodes[i].succ);
  }
  free(new_nodes);
  free(new_indegree);
  free(new_orphaned);
  free(new_pending);
  free(new_completed);
  free(new_state_snapshots);
  free(new_state_sizes);
  free(new_source_nodes);
  return false;
}

int apply_add_node(LiveGraph *lg, NodeVTable vtable, size_t state_size,
                   uint64_t logical_id, const char *name, int nInputs,
                   int nOutputs, const void *initial_state) {
  // Use logical_id directly as the array index
  int node_id = (int)logical_id;

  if (node_id >= lg->node_capacity) {
    // Need to expand capacity
    if (!grow_node_capacity(lg, node_id)) {
      return -1;
    }
  }

  // Allocate aligned memory for node state if size > 0
  void *state = NULL;
  if (state_size > 0) {
    assert((state_size % sizeof(float)) == 0);
    state = alloc_state_f32(state_size / sizeof(float), 64);
    if (!state) {
      return -1;
    }
    memset(state, 0, state_size); // Zero-initialize the state memory

    // Call NodeVTable init function if provided
    if (vtable.init) {
      int sr = g_engine.sampleRate > 0 ? g_engine.sampleRate : 48000;
      int bs = g_engine.blockSize > 0 ? g_engine.blockSize : 256;
      vtable.init(state, sr, bs, initial_state);
    }
  }

  RTNode *node = &lg->nodes[node_id];
  memset(node, 0, sizeof(RTNode));

  node->logical_id = logical_id;
  node->debug_name = name ? strdup(name) : NULL;
  node->vtable = vtable;
  node->state = state;
  node->state_size = state_size;
  node->succCount = 0;

  // Set port counts from command
  node->nInputs = nInputs;
  node->nOutputs = nOutputs;

  // Initialize port arrays to NULL first
  node->inEdgeId = NULL;
  node->outEdgeId = NULL;
  node->succ = NULL;

  // Initialize cached IO pointers (will be built lazily on first process)
  node->cached_inPtrs = NULL;
  node->cached_outPtrs = NULL;
  node->io_cache_valid = false;

  // Set up port arrays if needed
  if (!ensure_port_arrays(node)) {
    // Port array allocation failed - node is not usable
    return -1;
  }

  // Initialize orphaned state - new nodes with no connections start as orphaned
  // They will be marked as non-orphaned when they get connected to the signal
  // path
  lg->sched.is_orphaned[node_id] = true;

  // Update node_count to be highest allocated index + 1
  if (node_id >= lg->node_count) {
    lg->node_count = node_id + 1;
  }

  return node_id;
}

/**
 * Delete a node from the live graph, properly disconnecting all its
 * connections. This function:
 * 1. Disconnects all inbound connections to this node
 * 2. Disconnects all outbound connections from this node
 * 3. Frees the node's state memory and port arrays
 * 4. Updates orphaned status for the graph
 * Returns true if successful, false if node_id is invalid.
 */
bool apply_delete_node(LiveGraph *lg, int node_id) {
  bool result = apply_delete_node_internal(lg, node_id);
  if (result) {
    update_orphaned_status(lg);
  }
  return result;
}

// ===================== Cycle Prevention =====================
// DFS reachability: can we get from 'from' to 'target' via successor edges?
static bool can_reach(LiveGraph *lg, int from, int target, bool *visited) {
  if (from == target)
    return true;
  if (from < 0 || from >= lg->node_count)
    return false;
  if (visited[from])
    return false;
  visited[from] = true;

  RTNode *node = &lg->nodes[from];
  for (int i = 0; i < node->succCount; i++) {
    if (can_reach(lg, node->succ[i], target, visited))
      return true;
  }
  return false;
}

// Would connecting src_node → dst_node create a cycle?
// Checks if dst_node can already reach src_node via existing edges.
static bool would_create_cycle(LiveGraph *lg, int src_node, int dst_node) {
  if (src_node == dst_node)
    return true; // self-loop
  bool *visited = calloc(lg->node_count, sizeof(bool));
  if (!visited)
    return false; // alloc failure → fail-open
  bool found = can_reach(lg, dst_node, src_node, visited);
  free(visited);
  return found;
}

// Internal version that skips update_orphaned_status for batched operations
bool apply_connect_internal(LiveGraph *lg, int src_node, int src_port,
                            int dst_node, int dst_port) {
  // --- Validate nodes/ports ---
  if (!lg || src_node < 0 || src_node >= lg->node_count || dst_node < 0 ||
      dst_node >= lg->node_count) {
    return false;
  }

  RTNode *S = &lg->nodes[src_node];
  RTNode *D = &lg->nodes[dst_node];

  if (src_port < 0 || src_port >= S->nOutputs || dst_port < 0 ||
      dst_port >= D->nInputs) {
    return false;
  }

  // Validate nodes before ensuring port arrays
  if (!is_node_valid(lg, src_node) || !is_node_valid(lg, dst_node)) {
    return false;
  }

  if (!ensure_port_arrays(S) || !ensure_port_arrays(D)) {
    return false;
  }

  // Reject connections that would create a cycle
  if (would_create_cycle(lg, src_node, dst_node)) {
    return false;
  }

  int existing_eid = D->inEdgeId[dst_port];
  if (existing_eid == -1) {
    // Case 1: First producer → normal 1:1 connect
    int eid = S->outEdgeId[src_port];
    if (eid == -1) {
      eid = alloc_edge(lg);
      if (eid < 0)
        return false; // no capacity
      S->outEdgeId[src_port] = eid;
      lg->edges[eid].src_node = src_node;
      lg->edges[eid].src_port = src_port;
    }
    D->inEdgeId[dst_port] = eid;
    lg->edges[eid].refcount++;
    if (!has_successor(S, dst_node)) { // first S→D connection
      lg->sched.indegree[dst_node]++;        // count unique predecessor S
      add_successor_port(S, dst_node);
    } else {
      // successor already recorded; do NOT increment indegree again
    }
  } else {
    // Case 2 or 3: Already has a producer → use/create SUM(D, dst_port)
    int sum_id = D->fanin_sum_node_id[dst_port];
    if (sum_id == -1) {
      // Case 2: Create SUM with 2 inputs - allocate a fresh node ID.
      // NOTE: We must NOT scan for "free" slots in the existing node array,
      // because queued GE_ADD_NODE commands may have pre-allocated IDs for
      // slots that appear empty (zeroed by calloc) but are about to be
      // populated. Reusing such a slot would cause the SUM to be overwritten
      // when the queued add_node is processed later in the same drain.
      int free_id = atomic_fetch_add(&lg->next_node_id, 1);
      sum_id = apply_add_node(lg, SUM_VTABLE, 0, free_id, "SUM", 2, 1, NULL);
      if (sum_id < 0)
        return false;
      // Re-derive S and D: apply_add_node may have triggered
      // grow_node_capacity which reallocates lg->nodes, invalidating
      // the pointers captured at the top of this function.
      S = &lg->nodes[src_node];
      D = &lg->nodes[dst_node];
      RTNode *SUM = &lg->nodes[sum_id];
      if (!is_node_valid(lg, sum_id) || !ensure_port_arrays(SUM)) {
        return false;
      }

      // Find old source of existing_eid
      int old_src = lg->edges[existing_eid].src_node;
      int old_src_port = lg->edges[existing_eid].src_port;

      // Disconnect old_src → D:dst_port (lightweight local form)
      D->inEdgeId[dst_port] = -1;

      // FIX: Decrement refcount for removing D's consumption of existing_eid
      {
        LiveEdge *oe = &lg->edges[existing_eid];
        if (oe->refcount > 0)
          oe->refcount--; // remove D's consumption
      }

      // Only decrement indegree if old_src is no longer connected to dst_node
      // through any path
      if (!still_connected_S_to_D(lg, old_src, dst_node)) {
        if (lg->sched.indegree[dst_node] > 0)
          lg->sched.indegree[dst_node]--;
        // Remove dst_node from old_src's successor list since it's no longer a
        // direct successor
        remove_successor(&lg->nodes[old_src], dst_node);
      }

      // Hook old_src → SUM.in0 (reuse existing edge)
      SUM->inEdgeId[0] = existing_eid;
      lg->edges[existing_eid].refcount++; // SUM consumes it now
      indegree_inc_on_first_pred(lg, old_src, sum_id);

      // Ensure SUM has an output edge
      int sum_out = SUM->outEdgeId[0];
      if (sum_out == -1) {
        sum_out = alloc_edge(lg);
        if (sum_out < 0)
          return false;
        SUM->outEdgeId[0] = sum_out;
        lg->edges[sum_out].src_node = sum_id;
        lg->edges[sum_out].src_port = 0;
      }

      // New source S → SUM.in1
      int new_eid = S->outEdgeId[src_port];
      if (new_eid == -1) {
        new_eid = alloc_edge(lg);
        if (new_eid < 0)
          return false;
        S->outEdgeId[src_port] = new_eid;
        lg->edges[new_eid].src_node = src_node;
        lg->edges[new_eid].src_port = src_port;
      }
      SUM->inEdgeId[1] = new_eid;
      lg->edges[new_eid].refcount++;
      indegree_inc_on_first_pred(lg, src_node, sum_id);

      // SUM.out0 → D:dst_port
      D->inEdgeId[dst_port] = sum_out;
      lg->edges[sum_out].refcount++;
      indegree_inc_on_first_pred(lg, sum_id,
                                 dst_node); // Restore indegree since
                                            // destination now depends on SUM

      // Remember the SUM
      D->fanin_sum_node_id[dst_port] = sum_id;
    } else {
      // Case 3: SUM already exists → grow inputs by 1
      if (!is_sum_node_valid(lg, sum_id)) {
        // SUM node was deleted or is invalid - abort connection
        return false;
      }
      RTNode *SUM = &lg->nodes[sum_id];

      // Increase SUM->nInputs by 1 and resize its port arrays
      int newN = SUM->nInputs + 1;
      SUM->nInputs = newN;
      int32_t *new_ptr = realloc(SUM->inEdgeId, newN * sizeof(int32_t));
      if (!new_ptr) {
        // Realloc failed - restore original state
        SUM->nInputs = newN - 1;
        return false;
      }
      SUM->inEdgeId = new_ptr;
      SUM->inEdgeId[newN - 1] = -1; // init

      // Connect S → SUM.in(newN-1)
      int new_eid = S->outEdgeId[src_port];
      if (new_eid == -1) {
        new_eid = alloc_edge(lg);
        if (new_eid < 0)
          return false;
        S->outEdgeId[src_port] = new_eid;
        lg->edges[new_eid].src_node = src_node;
        lg->edges[new_eid].src_port = src_port;
      }
      SUM->inEdgeId[newN - 1] = new_eid;
      lg->edges[new_eid].refcount++;
      indegree_inc_on_first_pred(lg, src_node,
                                 sum_id); // ✅ only if first edge from S
    }
  }

  return true;
}

bool apply_connect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                   int dst_port) {
  bool result =
      apply_connect_internal(lg, src_node, src_port, dst_node, dst_port);
  if (result) {
    update_orphaned_status(lg);
  }
  return result;
}
