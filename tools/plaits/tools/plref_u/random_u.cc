// Test-only: PLREF_U=<u> -> constant GetFloat(); PLREF_SEED=<n> -> LCG seed.
#include <cstdlib>
#include "stmlib/utils/random.h"
namespace stmlib {
uint32_t Random::rng_state_ = getenv("PLREF_SEED") ? uint32_t(atol(getenv("PLREF_SEED"))) : 0x21;
float Random::const_u_ = getenv("PLREF_U") ? float(atof(getenv("PLREF_U"))) : -1.0f;
}
