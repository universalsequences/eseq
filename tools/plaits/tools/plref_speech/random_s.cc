#include <cstdlib>
#include "stmlib/utils/random.h"
namespace stmlib {
uint32_t Random::rng_state_ = 0x21;
float Random::const_u_ = getenv("PLREF_U") ? float(atof(getenv("PLREF_U"))) : -1.0f;
}
