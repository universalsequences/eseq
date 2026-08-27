# Makefile for multi-threaded audio graph system
# Builds the modular audio graph demo with proper C11 threading support

CC ?= cc
UNAME_S := $(shell uname -s)

CFLAGS ?= -O2
CFLAGS += -std=c11 -Wall -Wextra -pthread
LDLIBS += -lm

TARGET = audiograph

ifeq ($(UNAME_S),Darwin)
CFLAGS += -mmacosx-version-min=14.0
DYLIB_TARGET = libaudiograph.dylib
SHARED_LDFLAGS = -dynamiclib -install_name @rpath/libaudiograph.dylib \
	-compatibility_version 1.0 -current_version 1.0
else ifeq ($(UNAME_S),Linux)
CPPFLAGS += -D_GNU_SOURCE
CFLAGS += -fPIC
DYLIB_TARGET = libaudiograph.so
SHARED_LDFLAGS = -shared -Wl,-soname,libaudiograph.so
else
$(error Unsupported operating system: $(UNAME_S))
endif

# Source files for the modular build
SOURCES = main.c graph_nodes.c graph_engine.c graph_api.c graph_edit.c ready_queue.c hot_swap.c wrapper.c
OBJECTS = $(SOURCES:.c=.o)

# Library source files (exclude main.c)
LIB_SOURCES = graph_nodes.c graph_engine.c graph_api.c graph_edit.c ready_queue.c hot_swap.c wrapper.c
LIB_OBJECTS = $(LIB_SOURCES:.c=.o)

# Common object files needed by test binaries
TEST_OBJS = graph_engine.o graph_nodes.o graph_api.o graph_edit.o ready_queue.o hot_swap.o wrapper.o
TEST_SOURCES := $(wildcard tests/test_*.c)
TEST_BINARIES := $(TEST_SOURCES:.c=)

# Header dependencies
HEADERS = graph_types.h mpmc_queue.h graph_nodes.h graph_engine.h graph_api.h graph_edit.h hot_swap.h

# Default target
all: $(TARGET) $(DYLIB_TARGET)

# Build the main executable
$(TARGET): $(OBJECTS)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(TARGET) $(OBJECTS) $(LDLIBS)

# Build the platform's dynamic library.
$(DYLIB_TARGET): $(LIB_OBJECTS)
	$(CC) $(CFLAGS) $(LDFLAGS) $(SHARED_LDFLAGS) \
		-o $(DYLIB_TARGET) $(LIB_OBJECTS) $(LDLIBS)

# Build object files
%.o: %.c $(HEADERS)
	$(CC) $(CPPFLAGS) $(CFLAGS) -c $< -o $@

# Debug build with more verbose output and debug symbols
debug: CFLAGS += -g -DDEBUG -O0
debug: $(TARGET)

# Release build with optimizations
release: CFLAGS += -O3 -DNDEBUG
release: $(TARGET)

# Library-only build
lib: $(DYLIB_TARGET)

# Release library build with optimizations
lib-release: CFLAGS += -O3 -DNDEBUG
lib-release: $(DYLIB_TARGET)

# Run the demo
run: $(TARGET)
	./$(TARGET)

# Clean up build artifacts
clean: clean_tests
	rm -f $(TARGET) libaudiograph.dylib libaudiograph.so $(OBJECTS) a.out
	rm -rf $(TARGET).dSYM debug_iteration_* graph_test test_auto_sum test_sum_chain

# Check for memory leaks (macOS)
valgrind: $(TARGET)
	valgrind --leak-check=full --show-leak-kinds=all ./$(TARGET)

# Profile performance (requires Xcode tools on macOS)
profile: $(TARGET)
	instruments -t "Time Profiler" ./$(TARGET)

# Test targets
test: tests/test_watchlist tests/test_watchlist_advanced tests/test_watchlist_validation tests/test_watchlist_capacity_growth tests/test_watchlist_initial_capacity tests/test_watchlist_upstream_processing tests/test_mpmc_queue tests/test_live_graph_partial_connections tests/test_disconnect tests/test_graph_edit_queue tests/test_queue_api tests/test_capacity_growth tests/test_custom_engine_growth tests/test_simple_teardown tests/test_orphan_comprehensive tests/test_auto_sum tests/test_sum_slot_overwrite tests/test_ordered_sum_topology tests/test_sum_behavior tests/test_sum_high_fanin tests/test_dependency_rebuild_sum tests/test_param_update_bounds tests/test_hot_swap tests/test_multi_port_routing tests/test_complex_topology tests/test_4_node_topology tests/test_4_node_fuzz tests/test_param_updates tests/test_stereo tests/test_patch_disconnect_orphan tests/test_indegree_unique_pred tests/test_deadlock_disconnect tests/test_scheduler_queue_saturation tests/test_buffer_api tests/test_cycle_prevention tests/test_write_node_state tests/test_block_events
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
	./tests/test_dependency_rebuild_sum
	./tests/test_param_update_bounds
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
	./tests/test_scheduler_queue_saturation
	./tests/test_buffer_api
	./tests/test_cycle_prevention
	./tests/test_write_node_state
	./tests/test_block_events

DEBUG_TESTS = tests/test_dac_indegree_bug tests/test_graph_reuse_bug \
	tests/test_exact_bug_reproduction tests/test_bug_stress
PERFORMANCE_TESTS = tests/test_polysynth_scheduler_stress tests/test_fm_benchmark

# Tests share one link recipe so libraries always follow objects, as required by
# linkers that resolve archives and shared libraries from left to right.
$(TEST_BINARIES): %: %.c $(HEADERS) $(TEST_OBJS)
	$(CC) $(CPPFLAGS) $(CFLAGS) \
		$(if $(filter $@,$(DEBUG_TESTS)),-g -O0) \
		$(if $(filter $@,$(PERFORMANCE_TESTS)),-O3) \
		-I. $(LDFLAGS) -o $@ $< $(TEST_OBJS) $(LDLIBS)

clean_tests:
	rm -f $(TEST_BINARIES)
	rm -rf tests/*.dSYM

.PHONY: all debug release lib lib-release run clean valgrind profile test clean_tests
