#include "../graph_api.h"
#include "../graph_engine.h"
#include "../graph_types.h"

#include <assert.h>
#include <math.h>
#include <stdio.h>

#define TEST_EVENT_CAP 32

typedef struct {
  int count;
  int frames[TEST_EVENT_CAP];
} EventProbeState;

static void event_probe_begin(void *state, uint64_t block_serial,
                              int slice_start, int slice_nframes) {
  (void)block_serial;
  (void)slice_start;
  (void)slice_nframes;
  EventProbeState *s = (EventProbeState *)state;
  s->count = 0;
}

static bool event_probe_schedule(void *state, const GraphBlockEvent *event) {
  EventProbeState *s = (EventProbeState *)state;
  if (!event || s->count >= TEST_EVENT_CAP)
    return false;
  s->frames[s->count++] = (int)event->frame_offset;
  return true;
}

static void event_probe_process(float *const *in, float *const *out, int nframes,
                                void *state, void *buffers) {
  (void)in;
  (void)buffers;
  EventProbeState *s = (EventProbeState *)state;
  float *y = out[0];
  for (int i = 0; i < nframes; i++) {
    y[i] = 0.0f;
  }
  for (int i = 0; i < s->count; i++) {
    int frame = s->frames[i];
    if (frame >= 0 && frame < nframes) {
      y[frame] += 1.0f;
    }
  }
}

static const NodeVTable EVENT_PROBE_VTABLE = {
    .process = event_probe_process,
    .init = NULL,
    .reset = NULL,
    .migrate = NULL,
    .begin_event_slice = event_probe_begin,
    .schedule_event = event_probe_schedule,
};

static GraphBlockEvent event_for(uint64_t logical_id, uint32_t frame,
                                 uint32_t sequence) {
  GraphBlockEvent event = {0};
  event.logical_id = logical_id;
  event.frame_offset = frame;
  event.sequence = sequence;
  event.kind = GBE_PULSE;
  event.aux_count = 0;
  return event;
}

static void assert_impulses(const float *out, int frames, const int *expected,
                            int expected_count) {
  for (int i = 0; i < frames; i++) {
    bool should_fire = false;
    for (int j = 0; j < expected_count; j++) {
      if (expected[j] == i) {
        should_fire = true;
        break;
      }
    }
    if (should_fire) {
      assert(fabsf(out[i] - 1.0f) < 0.0001f);
    } else {
      assert(fabsf(out[i]) < 0.0001f);
    }
  }
}

static void block_events_rebase_across_internal_slices(void) {
  initialize_engine(128, 48000);
  LiveGraph *lg = create_live_graph(16, 128, "block_event_rebase", 1);
  assert(lg);

  int probe = add_node(lg, EVENT_PROBE_VTABLE, sizeof(EventProbeState),
                       "event_probe", 0, 1, NULL, 0);
  assert(probe > 0);
  assert(graph_connect(lg, probe, 0, lg->dac_node_id, 0));

  const int expected[] = {0, 127, 128, 255, 300, 511};
  for (int i = 0; i < (int)(sizeof(expected) / sizeof(expected[0])); i++) {
    assert(push_block_event(lg, event_for((uint64_t)probe, expected[i],
                                          (uint32_t)i)));
  }

  float out[512] = {0};
  process_next_block(lg, out, 512);
  assert_impulses(out, 512, expected,
                  (int)(sizeof(expected) / sizeof(expected[0])));
  destroy_live_graph(lg);
}

static void empty_slices_clear_stale_timeline(void) {
  initialize_engine(128, 48000);
  LiveGraph *lg = create_live_graph(16, 128, "block_event_empty_slice", 1);
  assert(lg);

  int probe = add_node(lg, EVENT_PROBE_VTABLE, sizeof(EventProbeState),
                       "event_probe", 0, 1, NULL, 0);
  assert(probe > 0);
  assert(graph_connect(lg, probe, 0, lg->dac_node_id, 0));
  assert(push_block_event(lg, event_for((uint64_t)probe, 0, 0)));

  float out[512] = {0};
  process_next_block(lg, out, 512);
  const int expected[] = {0};
  assert_impulses(out, 512, expected, 1);
  destroy_live_graph(lg);
}

int main(void) {
  block_events_rebase_across_internal_slices();
  empty_slices_clear_stale_timeline();
  printf("test_block_events passed\n");
  return 0;
}
