// Test-only override of stmlib/utils/random.h for the Rings reference
// renderer: RREF_U=<u> makes Random::GetFloat() return the constant u, so
// the noise exciters become deterministic (the port feeds the same
// constant). Unset, it is the stmlib LCG.
#ifndef STMLIB_UTILS_RANDOM_H_
#define STMLIB_UTILS_RANDOM_H_
#include <cstddef>
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
  static inline int16_t GetSample() { return static_cast<int16_t>(GetWord() >> 16); }
  static inline float GetFloat() {
    if (const_u_ >= 0.0f) return const_u_;
    return static_cast<float>(GetWord()) / 4294967296.0f;
  }
  // RREF_NOISE=<file>: float32 (u0, u1) per sample, the port's two noise
  // streams; GetFloatAt(stream, i) reads sample sample_base_ + i of one.
  static inline float GetFloatAt(int stream, size_t i) {
    if (noise_ && (sample_base_ + i) < noise_size_) return noise_[2 * (sample_base_ + i) + stream];
    return GetFloat();
  }
  static uint32_t rng_state_;
  static float const_u_;
  static const float* noise_;
  static size_t noise_size_;
  static size_t sample_base_;
};
}  // namespace stmlib
#endif
