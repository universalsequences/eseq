#ifndef GRAPH_PROFILE_H
#define GRAPH_PROFILE_H

#include "graph_engine.h"

#ifdef AUDIOGRAPH_EXPERIMENTS
#define GRAPH_PROFILE_NODES 4096
#define GRAPH_PROFILE_EDGES 32768
#define GRAPH_PROFILE_SLOTS 17
typedef struct {
  uint64_t start_ns, end_ns, logical_id;
  int worker;
  char name[96];
} GraphProfileNode;
typedef struct { int source, destination; } GraphProfileEdge;
typedef struct {
  uint64_t start_ns, end_ns;
  int frames, workers, node_count, edge_count, overflow;
  // When each execution slot joined the render session (0 = never).
  uint64_t worker_join_ns[GRAPH_PROFILE_SLOTS];
  GraphProfileNode nodes[GRAPH_PROFILE_NODES];
  GraphProfileEdge edges[GRAPH_PROFILE_EDGES];
} GraphProfile;

// One control-thread consumer. A request captures one render slice; take copies
// only after the callback has closed the worker session and published it.
bool graph_profile_request(void);
bool graph_profile_take(GraphProfile *output);
void graph_profile_begin(LiveGraph *graph, int frames);
uint64_t graph_profile_node_begin(void);
void graph_profile_node_end(int node, int worker, uint64_t start);
void graph_profile_end(LiveGraph *graph);
void graph_profile_worker_join(int slot);
void graph_profile_start_recorder(void);
void graph_profile_stop_recorder(void);
#else
static inline void graph_profile_begin(LiveGraph *g, int n) { (void)g; (void)n; }
static inline uint64_t graph_profile_node_begin(void) { return 0; }
static inline void graph_profile_node_end(int n, int w, uint64_t s) { (void)n; (void)w; (void)s; }
static inline void graph_profile_end(LiveGraph *g) { (void)g; }
static inline void graph_profile_worker_join(int s) { (void)s; }
static inline void graph_profile_start_recorder(void) {}
static inline void graph_profile_stop_recorder(void) {}
#endif
#endif
