#include <cstdlib>
#include "stmlib/utils/random.h"
namespace stmlib {
uint32_t Random::rng_state_ = 0x21;
uint32_t Random::counter_ = 0;
bool Random::det_ = getenv("ELREF_DET") && atoi(getenv("ELREF_DET")) != 0;
}
