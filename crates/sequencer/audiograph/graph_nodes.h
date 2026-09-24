#ifndef GRAPH_NODES_H
#define GRAPH_NODES_H

#include "graph_types.h"

// ===================== Node Memory Layout =====================

// Oscillator memory layout
#define OSC_MEMORY_SIZE 2
#define OSC_PHASE 0
#define OSC_INC 1

// Gain memory layout
#define GAIN_MEMORY_SIZE 1
#define GAIN_VALUE 0

// Number memory layout (outputs constant value)
#define NUMBER_MEMORY_SIZE 1
#define NUMBER_VALUE 0

// Mixer has no state
#define MIX_MEMORY_SIZE 0

// ===================== Node Processing Functions =====================

// Oscillator functions
void osc_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers);
void osc_migrate(void *newMemory, const void *oldMemory);

// Gain function
void gain_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers);

// Number function
void number_process(float *const *in, float *const *out, int n, void *memory,
                    void *buffers);

// Mixer functions
void mix2_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers);
void mix8_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers);

// DAC function (Digital-to-Analog Converter - final output sink)
void dac_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers);

// SUM function (Auto-summing for multiple edges into same input)
void sum_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers);

// ===================== Helper Functions =====================

// Get the number of inputs for the currently processing node
int ap_current_node_ninputs(void);
const float *ap_graph_node_state(uint64_t logical_id, int *out_slots);
uint32_t ap_current_node_io_generation(void);
/* Silence propagation for the running kernel; see graph_engine.c. */
int ap_inputs_silent(void);
void ap_set_output_silent(int port);
int ap_output_was_silent(int port);
void ap_emit_silence(float *const *out, int n);

// ===================== Node VTables =====================

extern const NodeVTable OSC_VTABLE;
extern const NodeVTable GAIN_VTABLE;
extern const NodeVTable NUMBER_VTABLE;
extern const NodeVTable MIX2_VTABLE;
extern const NodeVTable MIX8_VTABLE;
extern const NodeVTable DAC_VTABLE;
extern const NodeVTable SUM_VTABLE;

#endif // GRAPH_NODES_H
