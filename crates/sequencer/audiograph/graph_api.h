#ifndef GRAPH_API_H
#define GRAPH_API_H

#include "graph_engine.h"
#include "graph_types.h"

// ===================== Watch List API =====================
bool add_node_to_watchlist(LiveGraph *lg, int node_id);
bool remove_node_from_watchlist(LiveGraph *lg, int node_id);
void *get_node_state(LiveGraph *lg, int node_id, size_t *state_size);
bool get_node_state_into(LiveGraph *lg, int node_id, void *out,
                         size_t out_capacity, size_t *state_size);

// Debug
void debug_dump_graph(LiveGraph *lg);

#endif // GRAPH_API_H
