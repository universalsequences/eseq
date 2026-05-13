/******************************************************************************
 * AudioGraph Comprehensive Orphan Status Test
 *
 * Tests orphan detection across key scenarios with proper queue-based API
 *usage. Validates the back-pointer-based O(V+E) orphan detection algorithm.
 ******************************************************************************/

#include "graph_api.h"
#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static bool validate_specific_nodes(LiveGraph *lg, int *node_ids,
                                    bool *expected, int count,
                                    const char *test_name) {
  bool passed = true;

  for (int i = 0; i < count; i++) {
    int node_id = node_ids[i];
    bool actual = lg->sched.is_orphaned[node_id];
    bool expect = expected[i];

    if (actual != expect) {
      printf("❌ %s - Node %d: expected %s, got %s\n", test_name, node_id,
             expect ? "ORPHANED" : "CONNECTED",
             actual ? "ORPHANED" : "CONNECTED");
      passed = false;
    }
  }

  if (passed) {
    printf("✅ %s - All specified nodes have correct orphan status\n",
           test_name);
  }

  return passed;
}

static void print_all_nodes(LiveGraph *lg, const char *scenario) {
  printf("\n--- %s: %d nodes, DAC=%d ---\n", scenario, lg->node_count,
         lg->dac_node_id);
  for (int i = 0; i < lg->node_count; i++) {
    printf("  Node %d: %s\n", i, lg->sched.is_orphaned[i] ? "ORPHANED" : "CONNECTED");
  }
}

// Test 1: Basic connectivity
static bool test_basic_connectivity(void) {
  printf("\n=== Test 1: Basic Connectivity ===\n");

  LiveGraph *lg = create_live_graph(10, 128, "basic_test", 1);

  // Create: osc1 -> gain -> DAC, osc2 (orphaned)
  int osc1 = live_add_oscillator(lg, 440.0, "osc1");
  int osc2 = live_add_oscillator(lg, 880.0, "osc2");
  int gain = live_add_gain(lg, 0.5, "gain");
  int dac = lg->dac_node_id;

  // Connect osc1 path
  graph_connect(lg, osc1, 0, gain, 0);
  graph_connect(lg, gain, 0, dac, 0);
  // osc2 not connected

  apply_graph_edits(lg->graphEditQueue, lg);
  print_all_nodes(lg, "Basic Connectivity");

  // Test specific nodes
  int test_nodes[] = {dac, osc1, osc2, gain};
  bool expected[] = {false, false, true, false}; // DAC, osc1, osc2, gain

  bool result = validate_specific_nodes(lg, test_nodes, expected, 4,
                                        "Basic Connectivity");

  destroy_live_graph(lg);
  return result;
}

// Test 2: No DAC scenario
static bool test_no_dac(void) {
  printf("\n=== Test 2: No DAC ===\n");

  LiveGraph *lg = create_live_graph(10, 128, "no_dac_test", 1);

  int osc = live_add_oscillator(lg, 440.0, "osc");
  int gain = live_add_gain(lg, 0.5, "gain");

  graph_connect(lg, osc, 0, gain, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  print_all_nodes(lg, "With DAC Connected");

  // Test the actual "no DAC" scenario by deleting DAC node
  delete_node(lg, lg->dac_node_id);
  apply_graph_edits(lg->graphEditQueue, lg);

  print_all_nodes(lg, "After DAC Deletion");

  // All remaining nodes should be orphaned
  bool all_orphaned = true;
  for (int i = 0; i < lg->node_count; i++) {
    // Skip deleted DAC node
    if (i == 0)
      continue; // DAC was at node 0

    if (!lg->sched.is_orphaned[i]) {
      printf("❌ Node %d should be orphaned (DAC deleted)\n", i);
      all_orphaned = false;
    }
  }

  if (all_orphaned) {
    printf("✅ No DAC - All nodes correctly orphaned\n");
  }

  destroy_live_graph(lg);
  return all_orphaned;
}

// Test 3: Diamond topology
static bool test_diamond_topology(void) {
  printf("\n=== Test 3: Diamond Topology ===\n");

  LiveGraph *lg = create_live_graph(10, 128, "diamond_test", 1);

  // osc -> splits to gain1 & gain2 -> both feed mixer -> DAC
  int osc = live_add_oscillator(lg, 440.0, "osc");
  int gain1 = live_add_gain(lg, 0.3, "gain1");
  int gain2 = live_add_gain(lg, 0.7, "gain2");
  int mixer = live_add_mixer2(lg, "mixer");
  int dac = lg->dac_node_id;

  // Create diamond
  graph_connect(lg, osc, 0, gain1, 0);
  graph_connect(lg, osc, 0, gain2, 0);
  graph_connect(lg, gain1, 0, mixer, 0);
  graph_connect(lg, gain2, 0, mixer, 1);
  graph_connect(lg, mixer, 0, dac, 0);

  apply_graph_edits(lg->graphEditQueue, lg);
  print_all_nodes(lg, "Diamond Topology");

  // All nodes should be connected (none orphaned)
  int test_nodes[] = {dac, osc, gain1, gain2, mixer};
  bool expected[] = {false, false, false, false, false};

  bool result =
      validate_specific_nodes(lg, test_nodes, expected, 5, "Diamond Topology");

  destroy_live_graph(lg);
  return result;
}

// Test 4: Disconnection
static bool test_disconnection(void) {
  printf("\n=== Test 4: Disconnection ===\n");

  LiveGraph *lg = create_live_graph(10, 128, "disconnect_test", 1);

  int osc = live_add_oscillator(lg, 440.0, "osc");
  int gain = live_add_gain(lg, 0.5, "gain");
  int dac = lg->dac_node_id;

  // Initially connect
  graph_connect(lg, osc, 0, gain, 0);
  graph_connect(lg, gain, 0, dac, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  print_all_nodes(lg, "Before Disconnect");

  // Check all connected
  if (lg->sched.is_orphaned[dac] || lg->sched.is_orphaned[osc] || lg->sched.is_orphaned[gain]) {
    printf("❌ All nodes should be connected initially\n");
    destroy_live_graph(lg);
    return false;
  }

  // Disconnect osc from gain
  graph_disconnect(lg, osc, 0, gain, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  print_all_nodes(lg, "After Disconnect");

  // After disconnect: osc -> [broken] -> gain -> DAC
  // DAC starts the search and marks itself non-orphaned
  // DAC walks backwards to gain (gain -> DAC connection exists)
  // gain has no inputs, so DFS stops there
  // osc is not reachable from DAC, so it stays orphaned

  bool step2_passed = true;
  if (!lg->sched.is_orphaned[osc]) {
    printf("❌ osc should be orphaned (no path from osc to DAC)\n");
    step2_passed = false;
  }
  if (lg->sched.is_orphaned[gain]) {
    printf("❌ gain should NOT be orphaned (gain -> DAC path exists)\n");
    step2_passed = false;
  }
  if (lg->sched.is_orphaned[dac]) {
    printf("❌ DAC should NOT be orphaned (it's the search root)\n");
    step2_passed = false;
  }

  if (step2_passed) {
    printf("✅ Disconnection - Orphan status correct after disconnect\n");
  }

  destroy_live_graph(lg);
  return step2_passed;
}

// Test 5: Fan-out with partial connection
static bool test_fan_out(void) {
  printf("\n=== Test 5: Fan-out ===\n");

  LiveGraph *lg = create_live_graph(10, 128, "fanout_test", 1);

  // osc feeds 3 gains, but only 2 gains connect to mixer
  int osc = live_add_oscillator(lg, 440.0, "osc");
  int gain1 = live_add_gain(lg, 0.3, "gain1");
  int gain2 = live_add_gain(lg, 0.3, "gain2");
  int gain3 = live_add_gain(lg, 0.4, "gain3"); // Will be orphaned
  int mixer = live_add_mixer2(lg, "mixer");
  int dac = lg->dac_node_id;

  // Fan out to all gains
  graph_connect(lg, osc, 0, gain1, 0);
  graph_connect(lg, osc, 0, gain2, 0);
  graph_connect(lg, osc, 0, gain3, 0);

  // Only connect 2 gains to mixer
  graph_connect(lg, gain1, 0, mixer, 0);
  graph_connect(lg, gain2, 0, mixer, 1);
  // gain3 NOT connected to mixer

  graph_connect(lg, mixer, 0, dac, 0);

  apply_graph_edits(lg->graphEditQueue, lg);
  print_all_nodes(lg, "Fan-out");

  // gain3 should be orphaned, others connected
  int test_nodes[] = {dac, osc, gain1, gain2, gain3, mixer};
  bool expected[] = {false, false, false,
                     false, true,  false}; // Only gain3 orphaned

  bool result = validate_specific_nodes(lg, test_nodes, expected, 6, "Fan-out");

  destroy_live_graph(lg);
  return result;
}

// Performance test
static bool test_performance(void) {
  printf("\n=== Performance Test ===\n");

  LiveGraph *lg = create_live_graph(20, 128, "perf_test", 1);

  // Create larger graph
  int nodes[10];
  for (int i = 0; i < 10; i++) {
    nodes[i] = live_add_oscillator(lg, 440.0 + i * 40, "osc");
  }

  // Connect first 5 in chain to DAC, leave others orphaned
  for (int i = 0; i < 4; i++) {
    graph_connect(lg, nodes[i], 0, nodes[i + 1], 0);
  }
  graph_connect(lg, nodes[4], 0, lg->dac_node_id, 0);

  apply_graph_edits(lg->graphEditQueue, lg);

  // Measure performance of many orphan updates
  struct timespec start, end;
  clock_gettime(CLOCK_MONOTONIC, &start);

  for (int i = 0; i < 100; i++) {
    graph_disconnect(lg, nodes[0], 0, nodes[1], 0);
    graph_connect(lg, nodes[0], 0, nodes[1], 0);
    apply_graph_edits(lg->graphEditQueue, lg);
  }

  clock_gettime(CLOCK_MONOTONIC, &end);
  double duration_ms = (end.tv_sec - start.tv_sec) * 1000.0 +
                       (end.tv_nsec - start.tv_nsec) / 1000000.0;

  printf("100 orphan updates in %.3f ms (%.1f μs per update)\n", duration_ms,
         duration_ms * 10.0);
  printf("Graph: %d nodes - should be O(V+E) complexity\n", lg->node_count);

  destroy_live_graph(lg);
  return duration_ms < 50.0; // Should be very fast
}

int main(void) {
  printf("AudioGraph Comprehensive Orphan Status Test\n");
  printf("===========================================\n");
  printf("Testing orphan detection with proper queue-based API:\n");
  printf("- Basic connectivity patterns\n");
  printf("- No DAC scenarios\n");
  printf("- Diamond topology\n");
  printf("- Disconnection effects\n");
  printf("- Fan-out with partial connections\n");
  printf("- Performance scaling\n\n");

  struct {
    const char *name;
    bool (*test_func)(void);
  } tests[] = {{"Basic Connectivity", test_basic_connectivity},
               {"No DAC", test_no_dac},
               {"Diamond Topology", test_diamond_topology},
               {"Disconnection", test_disconnection},
               {"Fan-out", test_fan_out},
               {"Performance", test_performance}};

  int passed = 0;
  int total = sizeof(tests) / sizeof(tests[0]);

  for (int i = 0; i < total; i++) {
    printf("\n"
           "="
           "="
           "="
           " Running %s "
           "="
           "="
           "="
           "\n",
           tests[i].name);

    struct timespec start, end;
    clock_gettime(CLOCK_MONOTONIC, &start);

    bool result = tests[i].test_func();

    clock_gettime(CLOCK_MONOTONIC, &end);
    double duration = (end.tv_sec - start.tv_sec) * 1000.0 +
                      (end.tv_nsec - start.tv_nsec) / 1000000.0;

    if (result) {
      printf("✅ PASSED: %s (%.3f ms)\n", tests[i].name, duration);
      passed++;
    } else {
      printf("❌ FAILED: %s (%.3f ms)\n", tests[i].name, duration);
    }
  }

  printf("\n"
         "="
         "="
         "="
         " Test Summary "
         "="
         "="
         "="
         "\n");
  printf("Passed: %d / %d tests\n", passed, total);

  if (passed == total) {
    printf("\n🎉 ALL COMPREHENSIVE ORPHAN TESTS PASSED!\n");
    printf("Back-pointer orphan detection works correctly across scenarios.\n");
    return 0;
  } else {
    printf("\n💥 %d TESTS FAILED!\n", total - passed);
    return 1;
  }
}
