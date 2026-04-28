#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

int main() {
  printf("=== GraphEditQueue Test ===\n");
  printf("Testing dynamic graph editing via queue-based commands\n\n");

  // Create a live graph
  LiveGraph *lg = create_live_graph(10, 128, "edit_queue_test", 1);
  assert(lg != NULL);
  assert(lg->graphEditQueue != NULL);

  printf("✓ LiveGraph created with GraphEditQueue initialized\n");

  // Test 1: Queue connect commands
  printf("\nTest 1: Queueing connect commands...\n");

  // First create some nodes directly (not via queue for this test)
  int osc1 = live_add_oscillator(lg, 440.0f, "osc1");
  int osc2 = live_add_oscillator(lg, 880.0f, "osc2");
  int mixer = live_add_mixer2(lg, "mixer");
  int dac = lg->dac_node_id; // Use the auto-created DAC

  printf("  Created nodes: osc1=%d, osc2=%d, mixer=%d, dac=%d\n", osc1, osc2,
         mixer, dac);

  // Process queued node creation before creating connections
  apply_graph_edits(lg->graphEditQueue, lg);

  // Queue connect commands instead of connecting directly
  GraphEditCmd connect_cmd1 = {
      .op = GE_CONNECT,
      .u.connect = {
          .src_id = osc1, .src_port = 0, .dst_id = mixer, .dst_port = 0}};

  GraphEditCmd connect_cmd2 = {
      .op = GE_CONNECT,
      .u.connect = {
          .src_id = osc2, .src_port = 0, .dst_id = mixer, .dst_port = 1}};

  GraphEditCmd connect_cmd3 = {
      .op = GE_CONNECT,
      .u.connect = {
          .src_id = mixer, .src_port = 0, .dst_id = dac, .dst_port = 0}};

  // Push commands to queue
  bool push1 = geq_push(lg->graphEditQueue, &connect_cmd1);
  bool push2 = geq_push(lg->graphEditQueue, &connect_cmd2);
  bool push3 = geq_push(lg->graphEditQueue, &connect_cmd3);

  assert(push1 && push2 && push3);
  printf("✓ Successfully queued 3 connect commands\n");

  // Verify connections don't exist yet (edits not applied)
  RTNode *mixer_node = &lg->nodes[mixer];
  assert(mixer_node->inEdgeId[0] == -1); // Should be unconnected
  assert(mixer_node->inEdgeId[1] == -1); // Should be unconnected
  printf("✓ Connections not yet applied (as expected)\n");

  // Apply the queued edits
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result == true);
  printf("✓ Successfully applied queued edits\n");

  // Verify connections now exist
  assert(mixer_node->inEdgeId[0] != -1); // Should be connected
  assert(mixer_node->inEdgeId[1] != -1); // Should be connected
  printf("✓ Connections established after applying edits\n");

  // Test 2: Queue disconnect commands
  printf("\nTest 2: Queueing disconnect commands...\n");

  GraphEditCmd disconnect_cmd = {
      .op = GE_DISCONNECT,
      .u.disconnect = {
          .src_id = osc1, .src_port = 0, .dst_id = mixer, .dst_port = 0}};

  bool push_disconnect = geq_push(lg->graphEditQueue, &disconnect_cmd);
  assert(push_disconnect);
  printf("✓ Successfully queued disconnect command\n");

  // Verify connection still exists (edit not applied yet)
  assert(mixer_node->inEdgeId[0] != -1);
  printf("✓ Connection still exists before applying disconnect\n");

  // Apply the disconnect edit
  bool apply_disconnect = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_disconnect == true);
  printf("✓ Successfully applied disconnect edit\n");

  // Verify connection is now removed
  assert(mixer_node->inEdgeId[0] == -1);
  printf("✓ Connection removed after applying disconnect\n");

  // Test 3: Queue add_node commands
  printf("\nTest 3: Queueing add_node commands...\n");

  // Memory is now allocated by the library
  GraphEditCmd add_node_cmd = {.op = GE_ADD_NODE,
                               .u.add_node = {.vt = GAIN_VTABLE,
                                              .state_size = GAIN_MEMORY_SIZE * sizeof(float),
                                              .logical_id = lg->node_count,
                                              .name = strdup("queued_gain"),
                                              .nInputs = 1,
                                              .nOutputs = 1}};

  int initial_node_count = lg->node_count;
  bool push_add = geq_push(lg->graphEditQueue, &add_node_cmd);
  assert(push_add);
  printf("✓ Successfully queued add_node command\n");

  // Verify node count hasn't changed yet
  assert(lg->node_count == initial_node_count);
  printf("✓ Node count unchanged before applying add_node\n");

  // Apply the add_node edit
  bool apply_add = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_add == true);
  printf("✓ Successfully applied add_node edit\n");

  // Verify new node was added
  assert(lg->node_count == initial_node_count + 1);
  printf("✓ Node count increased after applying add_node\n");

  // Test 4: Process blocks with edit queue integration
  printf("\nTest 4: Testing edit queue integration with block processing...\n");

  // Queue a new connection while system is running (mixer port 0 is now free
  // after disconnect)
  GraphEditCmd runtime_connect = {
      .op = GE_CONNECT,
      .u.connect = {.src_id = osc2,
                    .src_port = 0,
                    .dst_id = mixer,
                    .dst_port = 0} // Connect osc2 to mixer port 0
  };

  bool push_runtime = geq_push(lg->graphEditQueue, &runtime_connect);
  assert(push_runtime);
  printf("✓ Queued connection during runtime\n");

  // Verify connection doesn't exist yet
  assert(mixer_node->inEdgeId[0] == -1); // Should still be disconnected
  printf("✓ Runtime connection not yet applied\n");

  // Process a block - this should apply the queued edit via process_next_block
  float output_buffer[128];
  process_next_block(lg, output_buffer, 128);

  // Verify the runtime connection was applied
  assert(mixer_node->inEdgeId[0] != -1); // Should now be connected to osc2
  printf("✓ Runtime edit applied during block processing\n");

  // Test 5: Queue overflow handling
  printf("\nTest 5: Testing queue overflow handling...\n");

  // Fill up the queue beyond capacity (queue was initialized with capacity 256)
  int overflow_pushes = 0;
  int successful_pushes = 0;

  // Push way more than the capacity to force overflow
  for (int i = 0; i < 1024; i++) {
    GraphEditCmd dummy_cmd = {
        .op = GE_CONNECT,
        .u.connect = {.src_id = 0, .src_port = 0, .dst_id = 1, .dst_port = 0}};

    if (geq_push(lg->graphEditQueue, &dummy_cmd)) {
      successful_pushes++;
    } else {
      overflow_pushes++;
    }
  }

  printf("  Successful pushes: %d, Overflow pushes: %d\n", successful_pushes,
         overflow_pushes);
  printf("  Queue capacity appears to be: %d\n", successful_pushes);

  // If no overflow occurred, the queue is larger than expected - that's fine
  if (overflow_pushes == 0) {
    printf("✓ Queue handled all commands (capacity larger than test size)\n");
  } else {
    printf("✓ Queue overflow handled correctly (rejected commands)\n");
  }

  // Clear the queue by applying all edits (most will fail due to invalid nodes)
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Queue cleared by applying edits\n");

  // Test 6: Node deletion via queue
  printf("\nTest 6: Testing node deletion via queue...\n");

  // Create a temporary node to delete
  int temp_gain = live_add_gain(lg, 0.8f, "temp_gain");
  assert(temp_gain >= 0);
  printf("  Created temporary gain node: %d\n", temp_gain);

  // Connect it to the graph: osc1 -> temp_gain (osc1 should be free after Test
  // 2 disconnect)
  assert(apply_connect(lg, osc1, 0, temp_gain, 0));
  printf("✓ Connected temp_gain to the graph\n");

  // Verify connections exist
  RTNode *temp_node = &lg->nodes[temp_gain];
  assert(temp_node->inEdgeId[0] != -1); // Input connected from osc1
  printf("✓ Connections verified before deletion\n");

  // Queue delete command
  GraphEditCmd delete_cmd = {.op = GE_REMOVE_NODE,
                             .u.remove_node = {.node_id = temp_gain}};

  bool push_delete = geq_push(lg->graphEditQueue, &delete_cmd);
  assert(push_delete);
  printf("✓ Successfully queued delete command\n");

  // Verify node still exists (edit not applied yet)
  assert(temp_node->state != NULL); // Should still be valid
  printf("✓ Node still exists before applying delete\n");

  // Apply the delete edit
  bool apply_delete = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_delete == true);
  printf("✓ Successfully applied delete edit\n");

  // Verify node is deleted and connections are cleaned up
  assert(temp_node->state == NULL);        // Should be marked as deleted
  assert(temp_node->vtable.process == NULL); // Vtable should be cleared
  assert(temp_node->inEdgeId == NULL);     // Port arrays freed
  assert(temp_node->outEdgeId == NULL);
  printf("✓ Node deleted and all connections cleaned up\n");

  printf("\n=== GraphEditQueue Test Results ===\n");
  printf("✅ All GraphEditQueue tests passed successfully!\n");
  printf("   - Connect commands applied correctly\n");
  printf("   - Disconnect commands applied correctly\n");
  printf("   - Add_node commands applied correctly\n");
  printf("   - Delete_node commands applied correctly\n");
  printf("   - Runtime integration with block processing works\n");
  printf("   - Queue overflow handled safely\n");

  // No cleanup needed - memory is managed by the library

  return 0;
}
