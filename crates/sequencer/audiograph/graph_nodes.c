#include "graph_nodes.h"

// ===================== Oscillator Implementation =====================

void osc_init(void *memory, int sr, int maxBlock, const void *initial_state) {
  (void)sr;
  (void)maxBlock;
  float *mem = (float *)memory;
  mem[OSC_PHASE] = 0.0f;

  // Set OSC_INC from initial_state if provided
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    mem[OSC_INC] = init_data[0]; // Frequency increment
  } else {
    mem[OSC_INC] = 0.0f; // Default to no oscillation
  }
}

void osc_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers) {
  (void)in;
  (void)buffers;
  float *mem = (float *)memory;
  float *y = out[0];

  for (int i = 0; i < n; i++) {
    y[i] = 2.0f * mem[OSC_PHASE] - 1.0f;
    mem[OSC_PHASE] += mem[OSC_INC];
    if (mem[OSC_PHASE] >= 1.f)
      mem[OSC_PHASE] -= 1.f;
  }
}

void osc_migrate(void *newMemory, const void *oldMemory) {
  const float *oldMem = (const float *)oldMemory;
  float *newMem = (float *)newMemory;
  newMem[OSC_PHASE] = oldMem[OSC_PHASE];
  // OSC_INC typically doesn't need migration (set during creation)
}

// ===================== Gain Implementation =====================

void gain_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers) {
  (void)buffers;
  float *mem = (float *)memory;
  float gain = mem[GAIN_VALUE];
  const float *a = in[0];
  float *y = out[0];
  for (int i = 0; i < n; i++)
    y[i] = a[i] * gain;
}

// ===================== Number Implementation =====================

void number_process(float *const *in, float *const *out, int n, void *memory,
                    void *buffers) {
  (void)in;
  (void)buffers;
  float *mem = (float *)memory;
  float value = mem[NUMBER_VALUE];
  float *y = out[0];
  for (int i = 0; i < n; i++)
    y[i] = value;
}

// ===================== Mixer Implementations =====================

void mix2_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers) {
  (void)memory;
  (void)buffers;
  const float *a = in[0];
  const float *b = in[1];
  float *y = out[0];
  // Use assignment (=) instead of addition to ensure clean output
  for (int i = 0; i < n; i++)
    y[i] = a[i] + b[i];
}

void mix8_process(float *const *in, float *const *out, int n, void *memory,
                  void *buffers) {
  (void)memory;
  (void)buffers;
  float *y = out[0];

  // Sum all 8 inputs
  for (int i = 0; i < n; i++) {
    y[i] = in[0][i] + in[1][i] + in[2][i] + in[3][i] + in[4][i] + in[5][i] +
           in[6][i] + in[7][i];
  }
}

void dac_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers) {
  (void)memory;
  (void)buffers;

  // DAC is a pass-through - copy all input channels to output channels
  // Number of channels is determined by the node's port configuration
  if (in && out) {
    // Get the number of inputs from the current processing context
    int num_channels = ap_current_node_ninputs();

    for (int ch = 0; ch < num_channels; ch++) {
      if (in[ch] && out[ch]) {
        for (int i = 0; i < n; i++) {
          out[ch][i] = in[ch][i]; // Pass each channel through
        }
      }
    }
  }
}

// ===================== SUM Implementation =====================

// Global accessor for current node's input count (defined in graph_engine.c)
extern int ap_current_node_ninputs(void);

void sum_process(float *const *in, float *const *out, int n, void *memory,
                 void *buffers) {
  (void)memory;
  (void)buffers;
  float *y = out[0];

  // Zero output buffer
  for (int i = 0; i < n; i++)
    y[i] = 0.0f;

  // Get number of inputs from the current processing context
  int nIn = ap_current_node_ninputs();
  // Accumulate all inputs
  for (int k = 0; k < nIn; k++) {
    const float *x = in[k];
    for (int i = 0; i < n; i++) {
      y[i] += x[i];
    }
  }
}

// ===================== Node VTables =====================

const NodeVTable OSC_VTABLE = {.process = osc_process,
                               .init = osc_init,
                               .reset = NULL,
                               .migrate = osc_migrate};

static void gain_init(void *state, int sampleRate, int maxBlock,
                      const void *initial_state) {
  (void)sampleRate;
  (void)maxBlock;
  float *memory = (float *)state;

  // Set gain value from initial_state if provided
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    memory[GAIN_VALUE] = init_data[0];
  } else {
    memory[GAIN_VALUE] = 1.0f; // Default gain of 1.0 (pass-through)
  }
}

const NodeVTable GAIN_VTABLE = {
    .process = gain_process, .init = gain_init, .reset = NULL, .migrate = NULL};

static void number_init(void *state, int sampleRate, int maxBlock,
                        const void *initial_state) {
  (void)sampleRate;
  (void)maxBlock;
  float *memory = (float *)state;

  // Set number value from initial_state if provided
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    memory[NUMBER_VALUE] = init_data[0];
  } else {
    memory[NUMBER_VALUE] = 0.0f; // Default number value of 0.0
  }
}

const NodeVTable NUMBER_VTABLE = {.process = number_process,
                                  .init = number_init,
                                  .reset = NULL,
                                  .migrate = NULL};

const NodeVTable MIX2_VTABLE = {
    .process = mix2_process, .init = NULL, .reset = NULL, .migrate = NULL};

const NodeVTable MIX8_VTABLE = {
    .process = mix8_process, .init = NULL, .reset = NULL, .migrate = NULL};

const NodeVTable DAC_VTABLE = {
    .process = dac_process, .init = NULL, .reset = NULL, .migrate = NULL};

const NodeVTable SUM_VTABLE = {
    .process = sum_process, .init = NULL, .reset = NULL, .migrate = NULL};
