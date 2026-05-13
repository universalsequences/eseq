#ifndef HOT_SWAP_H_
#define HOT_SWAP_H_

#include "graph_engine.h"

bool apply_hot_swap(LiveGraph *lg, GEHotSwapNode *op);
bool apply_replace_keep_edges(LiveGraph *lg, GEReplaceKeepEdges *op);

#endif // HOT_SWAP_H_
