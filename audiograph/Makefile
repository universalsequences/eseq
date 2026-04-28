# Makefile for multi-threaded audio graph system
# Builds the modular audio graph demo with proper C11 threading support

CC = gcc
MACOS_MIN = -mmacosx-version-min=14.0
CFLAGS = -std=c11 -O2 -Wall -Wextra -pthread $(MACOS_MIN)
TARGET = audiograph
DYLIB_TARGET = libaudiograph.dylib

# Source files for the modular build
SOURCES = main.c graph_nodes.c graph_engine.c graph_api.c graph_edit.c ready_queue.c hot_swap.c
OBJECTS = $(SOURCES:.c=.o)

# Library source files (exclude main.c)
LIB_SOURCES = graph_nodes.c graph_engine.c graph_api.c graph_edit.c ready_queue.c hot_swap.c
LIB_OBJECTS = $(LIB_SOURCES:.c=.o)

# Common object files needed by test binaries
TEST_OBJS = graph_engine.o graph_nodes.o graph_api.o graph_edit.o ready_queue.o hot_swap.o

# Header dependencies
HEADERS = graph_types.h mpmc_queue.h graph_nodes.h graph_engine.h graph_api.h graph_edit.h hot_swap.h

# Default target
all: $(TARGET) $(DYLIB_TARGET)

# Build the main executable
$(TARGET): $(OBJECTS)
	$(CC) $(CFLAGS) -o $(TARGET) $(OBJECTS)

# Build dynamic library for Swift integration
$(DYLIB_TARGET): $(LIB_OBJECTS)
	$(CC) $(CFLAGS) -dynamiclib -install_name @rpath/libaudiograph.dylib \
		-compatibility_version 1.0 -current_version 1.0 \
		-o $(DYLIB_TARGET) $(LIB_OBJECTS)

# Build object files
%.o: %.c $(HEADERS)
	$(CC) $(CFLAGS) -c $< -o $@

# Debug build with more verbose output and debug symbols
debug: CFLAGS += -g -DDEBUG -O0
debug: $(TARGET)

# Release build with optimizations
release: CFLAGS += -O3 -DNDEBUG
release: $(TARGET)

# Library-only build (just the dylib)
lib: $(DYLIB_TARGET)

# Release library build with optimizations
lib-release: CFLAGS += -O3 -DNDEBUG
lib-release: $(DYLIB_TARGET)

# Run the demo
run: $(TARGET)
	./$(TARGET)

# Clean up build artifacts
clean:
	rm -f $(TARGET) $(DYLIB_TARGET) $(OBJECTS) a.out
	rm -rf $(TARGET).dSYM debug_iteration_* graph_test test_auto_sum test_sum_chain

# Check for memory leaks (macOS)
valgrind: $(TARGET)
	valgrind --leak-check=full --show-leak-kinds=all ./$(TARGET)

# Profile performance (requires Xcode tools on macOS)
profile: $(TARGET)
	instruments -t "Time Profiler" ./$(TARGET)

# Test targets
test: tests/test_watchlist tests/test_watchlist_advanced tests/test_watchlist_validation tests/test_watchlist_capacity_growth tests/test_watchlist_initial_capacity tests/test_watchlist_upstream_processing tests/test_mpmc_queue tests/test_live_graph_partial_connections tests/test_disconnect tests/test_graph_edit_queue tests/test_queue_api tests/test_capacity_growth tests/test_custom_engine_growth tests/test_simple_teardown tests/test_orphan_comprehensive tests/test_auto_sum tests/test_sum_slot_overwrite tests/test_ordered_sum_topology tests/test_sum_behavior tests/test_sum_high_fanin tests/test_hot_swap tests/test_multi_port_routing tests/test_complex_topology tests/test_4_node_topology tests/test_4_node_fuzz tests/test_param_updates tests/test_stereo tests/test_patch_disconnect_orphan tests/test_indegree_unique_pred tests/test_deadlock_disconnect tests/test_buffer_api tests/test_cycle_prevention
	./tests/test_watchlist
	./tests/test_watchlist_advanced
	./tests/test_watchlist_validation
	./tests/test_watchlist_capacity_growth
	./tests/test_watchlist_initial_capacity
	./tests/test_watchlist_upstream_processing
	./tests/test_mpmc_queue
	./tests/test_live_graph_partial_connections
	./tests/test_disconnect
	./tests/test_graph_edit_queue
	./tests/test_queue_api
	./tests/test_capacity_growth
	./tests/test_custom_engine_growth
	./tests/test_simple_teardown
	./tests/test_orphan_comprehensive
	./tests/test_auto_sum
	./tests/test_sum_slot_overwrite
	./tests/test_ordered_sum_topology
	./tests/test_sum_behavior
	./tests/test_sum_high_fanin
	./tests/test_hot_swap
	./tests/test_multi_port_routing
	./tests/test_complex_topology
	./tests/test_4_node_topology
	./tests/test_4_node_fuzz
	./tests/test_param_updates
	./tests/test_stereo
	./tests/test_patch_disconnect_orphan
	./tests/test_indegree_unique_pred
	./tests/test_deadlock_disconnect
	./tests/test_buffer_api
	./tests/test_cycle_prevention

# Build MPMC queue unit tests
tests/test_mpmc_queue: tests/test_mpmc_queue.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_mpmc_queue tests/test_mpmc_queue.c $(TEST_OBJS)

# Build live graph partial connections test (orphaned nodes test)
tests/test_live_graph_partial_connections: tests/test_live_graph_partial_connections.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_live_graph_partial_connections tests/test_live_graph_partial_connections.c $(TEST_OBJS)

# Build disconnect test (port-based disconnect functionality)
tests/test_disconnect: tests/test_disconnect.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_disconnect tests/test_disconnect.c $(TEST_OBJS)

# Build graph edit queue test (dynamic editing via queue)
tests/test_graph_edit_queue: tests/test_graph_edit_queue.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_graph_edit_queue tests/test_graph_edit_queue.c $(TEST_OBJS)

# Build queue API test (pre-allocated IDs API)
tests/test_queue_api: tests/test_queue_api.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_queue_api tests/test_queue_api.c $(TEST_OBJS)

# Build deletion safety test (worker thread safety with node deletion)
tests/test_deletion_safety: tests/test_deletion_safety.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_deletion_safety tests/test_deletion_safety.c $(TEST_OBJS)

# Build capacity growth test (dynamic node array expansion)
tests/test_capacity_growth: tests/test_capacity_growth.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_capacity_growth tests/test_capacity_growth.c $(TEST_OBJS)

tests/test_custom_engine_growth: tests/test_custom_engine_growth.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_custom_engine_growth tests/test_custom_engine_growth.c $(TEST_OBJS)

# Build simple teardown test (basic graph destruction)
tests/test_simple_teardown: tests/test_simple_teardown.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_simple_teardown tests/test_simple_teardown.c $(TEST_OBJS)

# Build comprehensive orphan test (focused orphan status validation)
tests/test_orphan_comprehensive: tests/test_orphan_comprehensive.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_orphan_comprehensive tests/test_orphan_comprehensive.c $(TEST_OBJS)

# Build auto-sum test (automatic summing of multiple edges)
tests/test_auto_sum: tests/test_auto_sum.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_auto_sum tests/test_auto_sum.c $(TEST_OBJS)

tests/test_sum_slot_overwrite: tests/test_sum_slot_overwrite.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_sum_slot_overwrite tests/test_sum_slot_overwrite.c $(TEST_OBJS)

# Build auto-sum test (automatic summing of multiple edges)
tests/test_ordered_sum_topology: tests/test_ordered_sum_topology.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_ordered_sum_topology tests/test_ordered_sum_topology.c $(TEST_OBJS)

# Build sum behavior test (verify actual audio summing)
tests/test_sum_behavior: tests/test_sum_behavior.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_sum_behavior tests/test_sum_behavior.c $(TEST_OBJS)

# Build high fan-in sum stress test
tests/test_sum_high_fanin: tests/test_sum_high_fanin.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_sum_high_fanin tests/test_sum_high_fanin.c $(TEST_OBJS)

# Build hot swap test (verify hot swap functionality)
tests/test_hot_swap: tests/test_hot_swap.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_hot_swap tests/test_hot_swap.c $(TEST_OBJS)

# Build number node test (verify NUMBER node functionality)
tests/test_number_node: tests/test_number_node.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_number_node tests/test_number_node.c $(TEST_OBJS)

# Build parameter updates test (verify params_push functionality)
tests/test_param_updates: tests/test_param_updates.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_param_updates tests/test_param_updates.c $(TEST_OBJS)

# Build multi-port routing test (verify multi-output to multi-input connections)
tests/test_multi_port_routing: tests/test_multi_port_routing.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_multi_port_routing tests/test_multi_port_routing.c $(TEST_OBJS)

# Build complex topology test (specific multi-input/output graph structure validation)
tests/test_complex_topology: tests/test_complex_topology.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_complex_topology tests/test_complex_topology.c $(TEST_OBJS)

# Build 4-node topology test (reproduce edge deletion bug in specific topology)
tests/test_4_node_topology: tests/test_4_node_topology.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_4_node_topology tests/test_4_node_topology.c $(TEST_OBJS)

# Build 4-node fuzz test (exhaustive edge disconnection permutation testing)
tests/test_4_node_fuzz: tests/test_4_node_fuzz.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_4_node_fuzz tests/test_4_node_fuzz.c $(TEST_OBJS)

# Build DAC indegree bug reproduction test (isolated case from fuzz test)
tests/test_dac_indegree_bug: tests/test_dac_indegree_bug.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -g -O0 -I. -o tests/test_dac_indegree_bug tests/test_dac_indegree_bug.c $(TEST_OBJS)

# Build graph reuse state corruption test (many operations on same graph instance)
tests/test_graph_reuse_bug: tests/test_graph_reuse_bug.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -g -O0 -I. -o tests/test_graph_reuse_bug tests/test_graph_reuse_bug.c $(TEST_OBJS)

# Build disconnection bug reproduction test (minimal case from fuzz test findings)
tests/test_disconnection_bug: tests/test_disconnection_bug.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_disconnection_bug tests/test_disconnection_bug.c $(TEST_OBJS)

# Build exact bug reproduction test (exact copy of failing fuzz test case for lldb debugging)
tests/test_exact_bug_reproduction: tests/test_exact_bug_reproduction.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -g -O0 -I. -o tests/test_exact_bug_reproduction tests/test_exact_bug_reproduction.c $(TEST_OBJS)

# Build bug stress test (multiple iterations to trigger non-deterministic bug)
tests/test_bug_stress: tests/test_bug_stress.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -g -O0 -I. -o tests/test_bug_stress tests/test_bug_stress.c $(TEST_OBJS)

# Build multi-port auto-sum disconnect test (verify auto-sum disconnection doesn't cause dropouts)
tests/test_multiport_autosum_disconnect: tests/test_multiport_autosum_disconnect.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_multiport_autosum_disconnect tests/test_multiport_autosum_disconnect.c $(TEST_OBJS)

# Build new worker system test (validate block-boundary wake system)
tests/test_new_worker_system: tests/test_new_worker_system.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_new_worker_system tests/test_new_worker_system.c $(TEST_OBJS)

# Build watchlist tests (node state monitoring functionality)
tests/test_watchlist: tests/test_watchlist.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist tests/test_watchlist.c $(TEST_OBJS)

tests/test_watchlist_advanced: tests/test_watchlist_advanced.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist_advanced tests/test_watchlist_advanced.c $(TEST_OBJS)

tests/test_watchlist_validation: tests/test_watchlist_validation.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist_validation tests/test_watchlist_validation.c $(TEST_OBJS)

tests/test_watchlist_capacity_growth: tests/test_watchlist_capacity_growth.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist_capacity_growth tests/test_watchlist_capacity_growth.c $(TEST_OBJS)

tests/test_watchlist_initial_capacity: tests/test_watchlist_initial_capacity.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist_initial_capacity tests/test_watchlist_initial_capacity.c $(TEST_OBJS)

# Build watchlist upstream propagation test (watched nodes keep their inputs active)
tests/test_watchlist_upstream_processing: tests/test_watchlist_upstream_processing.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_watchlist_upstream_processing tests/test_watchlist_upstream_processing.c $(TEST_OBJS)

tests/test_stereo: tests/test_stereo.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_stereo tests/test_stereo.c $(TEST_OBJS)

# Build patch disconnect orphan test (reproduces the requested patch behavior)
tests/test_patch_disconnect_orphan: tests/test_patch_disconnect_orphan.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_patch_disconnect_orphan tests/test_patch_disconnect_orphan.c $(TEST_OBJS)

# Build indegree unique-predecessor test
tests/test_indegree_unique_pred: tests/test_indegree_unique_pred.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_indegree_unique_pred tests/test_indegree_unique_pred.c $(TEST_OBJS)

# Build worker-deadlock prevention test
tests/test_deadlock_disconnect: tests/test_deadlock_disconnect.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_deadlock_disconnect tests/test_deadlock_disconnect.c $(TEST_OBJS)

# Build buffer API test (create_buffer and hot_swap_buffer functionality)
tests/test_buffer_api: tests/test_buffer_api.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_buffer_api tests/test_buffer_api.c $(TEST_OBJS)

# Build cycle prevention test
tests/test_cycle_prevention: tests/test_cycle_prevention.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -I. -o tests/test_cycle_prevention tests/test_cycle_prevention.c $(TEST_OBJS)

# Build FM synthesis threading benchmark
tests/test_fm_benchmark: tests/test_fm_benchmark.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CFLAGS) -O3 -I. -o tests/test_fm_benchmark tests/test_fm_benchmark.c $(TEST_OBJS)

# Clean up test artifacts
clean: clean_tests

clean_tests:
	rm -f tests/test_watchlist tests/test_watchlist_advanced tests/test_watchlist_validation tests/test_watchlist_capacity_growth tests/test_watchlist_initial_capacity tests/test_watchlist_upstream_processing tests/test_mpmc_queue tests/test_engine_workers tests/test_live_graph_multithreaded tests/test_live_graph_workers tests/test_live_graph_partial_connections tests/test_disconnect tests/test_graph_edit_queue tests/test_queue_api tests/test_deletion_safety tests/test_capacity_growth tests/test_simple_teardown tests/test_orphan_comprehensive tests/test_auto_sum tests/test_sum_behavior tests/test_sum_high_fanin tests/test_engine_workers_debug tests/test_number_node tests/test_orphan_edge_cases tests/test_new_worker_system tests/test_hot_swap tests/test_multi_port_routing tests/test_complex_topology tests/test_4_node_topology tests/test_4_node_fuzz tests/test_param_updates tests/test_stereo tests/test_patch_disconnect_orphan tests/test_indegree_unique_pred tests/test_deadlock_disconnect tests/test_buffer_api tests/test_cycle_prevention
	rm -rf tests/*.dSYM

.PHONY: all debug release lib lib-release run clean valgrind profile test clean_tests
