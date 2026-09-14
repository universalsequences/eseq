// Loaded after process startup, like a compiled DGen instrument. Keep calls
// observable even in optimized builds and exercise allocation AND destruction.
#include <stdlib.h>
#ifdef __APPLE__
#include <malloc/malloc.h>
#endif

void eseq_allocation_control(unsigned operation) {
  void *pointer = NULL;
  switch (operation) {
  case 0:
    pointer = malloc(1357);
    break;
  case 1:
    pointer = calloc(1, 1357);
    break;
  case 2:
    pointer = malloc(1357);
    pointer = realloc(pointer, 2468);
    break;
  case 3:
    if (posix_memalign(&pointer, 64, 1357) != 0)
      abort();
    break;
#ifdef __APPLE__
  case 4:
    pointer = malloc_zone_malloc(malloc_default_zone(), 1357);
    *(volatile char *)pointer = 1;
    malloc_zone_free(malloc_default_zone(), pointer);
    return;
#endif
  default:
    abort();
  }
  if (!pointer)
    abort();
  *(volatile char *)pointer = 1;
  free(pointer);
}
