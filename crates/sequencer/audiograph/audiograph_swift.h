#ifndef AUDIOGRAPH_SWIFT_H
#define AUDIOGRAPH_SWIFT_H

// Audiograph Swift Integration Header
// Include this header in your Swift bridging header for audiograph integration
//
// Features included:
// - Real-time audio graph processing with worker thread pool
// - Dynamic node creation, connection, and deletion
// - Hot-swappable node replacement for seamless audio updates
// - Thread-safe parameter updates during processing
// - Watch list system for real-time node state monitoring
// - Multi-channel audio support (mono, stereo, or more channels)

#include "graph_api.h"
#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include "graph_types.h"

// ===================== Core Engine Functions =====================

// Initialize the audio engine (call once at app startup)
void initialize_engine(int block_size, int sample_rate);

// Start/stop worker threads for parallel processing
void engine_start_workers(int workers);
void engine_stop_workers(void);

// Optional: Join using OS Workgroup (kAudioOutputUnitProperty_OSWorkgroup)
void engine_set_os_workgroup(void *oswg);
void engine_clear_os_workgroup(void);

// Optional: enable minimal logging from RT workers (join success/failure)
void engine_enable_rt_logging(int enable);

// Enable/disable Mach time-constraint scheduling for workers (Apple only)
void engine_enable_rt_time_constraint(int enable);

// ===================== Live Graph Management =====================

// Create and destroy live graphs
//
// Parameters:
//   initial_capacity: Initial node capacity (grows automatically)
//   block_size: Audio block size in frames
//   label: Debug label for the graph
//   num_channels: Number of output channels (1=mono, 2=stereo, etc.)
//
// The output buffer for process_next_block() should be sized:
//   buffer_size = nframes * num_channels (interleaved format)
//
// Example for stereo:
//   LiveGraph *lg = create_live_graph(16, 128, "stereo_graph", 2);
//   float output[128 * 2];  // 128 frames * 2 channels
//   process_next_block(lg, output, 128);
//   // Output format: [L₀, R₀, L₁, R₁, L₂, R₂, ...]
LiveGraph *create_live_graph(int initial_capacity, int block_size,
                             const char *label, int num_channels);
void destroy_live_graph(LiveGraph *lg);

// ===================== Node Management =====================

// Generic node creation (returns pre-allocated node ID)
int add_node(LiveGraph *lg, NodeVTable vtable, size_t state_size,
             const char *name, int nInputs, int nOutputs,
             const void *initial_state, size_t initial_state_size);

// Convenient factory functions for common node types
int live_add_oscillator(LiveGraph *lg, float freq_hz, const char *name);
int live_add_gain(LiveGraph *lg, float gain_value, const char *name);
int live_add_number(LiveGraph *lg, float value, const char *name);
int live_add_mixer2(LiveGraph *lg, const char *name);
int live_add_mixer8(LiveGraph *lg, const char *name);
int live_add_sum(LiveGraph *lg, const char *name, int nInputs);

// Node deletion
bool delete_node(LiveGraph *lg, int node_id);

// Check if a node creation failed
bool is_failed_node(LiveGraph *lg, int node_id);

// ===================== Port-based Connections =====================

// Connect specific ports between nodes
bool graph_connect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                   int dst_port);

// Disconnect specific port connections
bool graph_disconnect(LiveGraph *lg, int src_node, int src_port, int dst_node,
                      int dst_port);

// Number of producer commands that can be queued without overflowing.
uint32_t graph_edit_queue_available(const LiveGraph *lg);

bool hot_swap_node(LiveGraph *lg, int node_id, NodeVTable vt, size_t state_size,
                   int nin, int nout, bool xfade,
                   void (*migrate)(void *, void *), const void *initial_state,
                   size_t initial_state_size);

bool replace_keep_edges(LiveGraph *lg, int node_id, NodeVTable vt,
                        size_t state_size, int nin, int nout, bool xfade,
                        void (*migrate)(void *, void *), const void *initial_state,
                        size_t initial_state_size);

// ===================== Real-time Audio Processing =====================

// Process one audio block (thread-safe, real-time safe)
//
// Parameters:
//   lg: LiveGraph instance
//   output_buffer: Output buffer for interleaved multi-channel audio
//                  Size must be: nframes * num_channels
//   nframes: Number of frames to process
//
// Output format is interleaved:
//   Mono:   [S₀, S₁, S₂, ...]
//   Stereo: [L₀, R₀, L₁, R₁, L₂, R₂, ...]
//   Quad:   [C₀₀, C₀₁, C₀₂, C₀₃, C₁₀, C₁₁, ...]
//
// Unconnected channels output silence (0.0f)
void process_next_block(LiveGraph *lg, float *output_buffer, int nframes);

// ===================== Parameter Updates =====================

// Thread-safe parameter updates (non-blocking)
bool params_push(ParamRing *r, ParamMsg m);
bool push_block_event(LiveGraph *lg, GraphBlockEvent event);

// ===================== Watch List API =====================

// Add a node to the watch list for state monitoring
bool add_node_to_watchlist(LiveGraph *lg, int node_id);

// Remove a node from the watch list
bool remove_node_from_watchlist(LiveGraph *lg, int node_id);

// Get a copy of a watched node's current state (caller must free the result)
// Returns NULL if node is not watched or doesn't exist
// If state_size is not NULL, it will be set to the size of the returned state
void *get_node_state(LiveGraph *lg, int node_id, size_t *state_size);
bool get_node_state_into(LiveGraph *lg, int node_id, void *out,
                         size_t out_capacity, size_t *state_size);

// ===================== Buffer API =====================

// Create a buffer with optional initial data
// Parameters:
//   lg: LiveGraph instance
//   size: Number of samples per channel
//   channel_count: Number of channels in the buffer
//   source_data: Initial data to copy (can be NULL for empty buffer)
//                Format: interleaved [ch0_s0, ch1_s0, ch0_s1, ch1_s1, ...]
//                Size should be: size * channel_count floats
// Returns buffer_id on success, -1 on failure
// Note: source_data is copied - caller can free it immediately after this call
int create_buffer(LiveGraph *lg, int size, int channel_count,
                  const float *source_data);

// Hot-swap buffer contents with new data
// Parameters:
//   lg: LiveGraph instance
//   buffer_id: ID of buffer to update (from create_buffer)
//   source_data: New data to copy into buffer
//   size: Number of samples per channel
//   channel_count: Number of channels
// Returns true on success, false if buffer doesn't exist or on failure
// Note: source_data is copied - caller can free it immediately after this call
int hot_swap_buffer(LiveGraph *lg, int buffer_id, const float *source_data,
                    int size, int channel_count);

// ===================== Node VTables for Custom Nodes =====================

extern const NodeVTable OSC_VTABLE;
extern const NodeVTable GAIN_VTABLE;
extern const NodeVTable NUMBER_VTABLE;
extern const NodeVTable MIX2_VTABLE;
extern const NodeVTable MIX8_VTABLE;
extern const NodeVTable DAC_VTABLE;
extern const NodeVTable SUM_VTABLE;

#endif // AUDIOGRAPH_SWIFT_H
