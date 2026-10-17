// Test-only override of stmlib/utils/random.h for the string/modal engine
// ports: PLREF_U=<u> makes Random::GetFloat() return the constant u so the
// noise exciters become deterministic (the port feeds the same constant).
#ifndef STMLIB_UTILS_RANDOM_H_
#define STMLIB_UTILS_RANDOM_H_
#include "stmlib/stmlib.h"
namespace stmlib {
class Random {
 public:
  static inline uint32_t state() { return rng_state_; }
  static inline void Seed(uint32_t seed) { rng_state_ = seed; }
  static inline uint32_t GetWord() {
    rng_state_ = rng_state_ * 1664525L + 1013904223L;
    return state();
  }
  static inline int16_t GetSample() { if (const_u_ >= 0.0f) return const_u_ > 0.5f ? 1 : -1; return static_cast<int16_t>(GetWord() >> 16); }
  static inline float GetFloat() {
    if (const_u_ >= 0.0f) return const_u_;
    return static_cast<float>(GetWord()) / 4294967296.0f;
  }
  static uint32_t rng_state_;
  static float const_u_;
};
}  // namespace stmlib
#endif
