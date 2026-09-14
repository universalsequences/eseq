#include "../graph_engine.h"
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#ifdef __APPLE__
#include <os/workgroup.h>
#endif

static EngineWorkgroupStatus status(void) {
  EngineWorkgroupStatus result;
  engine_get_workgroup_status(&result);
  return result;
}

int main(void) {
  initialize_engine(512, 48000);
  const int pools[] = {0, 1, 4};
  for (unsigned p = 0; p < sizeof(pools) / sizeof(pools[0]); ++p) {
    engine_start_workers(pools[p]);
#ifdef __APPLE__
    for (int cycle = 0; cycle < 20; ++cycle) {
      os_workgroup_t first = os_workgroup_parallel_create("eseq-first", NULL);
      os_workgroup_t second = os_workgroup_parallel_create("eseq-second", NULL);
      assert(first && second);
      engine_set_os_workgroup(first);
      EngineWorkgroupStatus joined = status();
      assert(joined.supported && joined.assigned);
      assert(joined.joined_workers == pools[p]);
      assert(!joined.failed_workers && !joined.pending_workers);
      // Re-reading the same retained property must not churn membership.
      engine_set_os_workgroup(first);
      assert(status().generation == joined.generation);
      os_release(first); // engine now owns the only application reference
      engine_set_os_workgroup(second);
      assert(status().joined_workers == pools[p]);
      assert(status().generation == joined.generation + 1);
      os_release(second);
      engine_clear_os_workgroup();
      assert(!status().assigned && !status().joined_workers);

      // Acknowledging a failed join must never be reported as membership.
      os_workgroup_t cancelled = os_workgroup_parallel_create("eseq-cancelled", NULL);
      assert(cancelled);
      os_workgroup_cancel(cancelled);
      engine_set_os_workgroup(cancelled);
      EngineWorkgroupStatus failed = status();
      assert(failed.failed_workers == pools[p] && !failed.joined_workers);
      assert(!failed.pending_workers);
      if (pools[p]) assert(failed.first_error == EINVAL);
      os_release(cancelled);
      engine_clear_os_workgroup();
    }
    os_workgroup_t final = os_workgroup_parallel_create("eseq-stop", NULL);
    assert(final);
    engine_set_os_workgroup(final);
    os_release(final);
#else
    assert(!status().supported);
#endif
    engine_stop_workers(); // must also release the final binding
    assert(!status().assigned && status().worker_count == 0);
    engine_clear_os_workgroup(); // safe after synchronization objects are gone
  }
  puts("PASS workgroup join, replace, failure, clear and pool restart");
  return 0;
}
