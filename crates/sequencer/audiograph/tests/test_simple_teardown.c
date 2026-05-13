#include "graph_engine.h"
#include <stdio.h>

int main() {
  printf("=== Simple Teardown Test ===\n");
  
  LiveGraph *lg = create_live_graph(4, 128, "test", 1);
  printf("Graph created\n");
  
  destroy_live_graph(lg);
  printf("Graph destroyed successfully\n");
  
  return 0;
}