// Test override of stmlib/utils/random.h for the Elements reference
// renderer: with ELREF_DET=1 every draw comes from a scrambled counter
// sequence that the DGenLisp test driver reproduces exactly; otherwise the
// stock LCG.
#ifndef STMLIB_UTILS_RANDOM_H_
#define STMLIB_UTILS_RANDOM_H_

#include "stmlib/stmlib.h"

namespace stmlib {

class Random {
 public:
  static inline uint32_t state() { return rng_state_; }
  static inline void Seed(uint32_t seed) { rng_state_ = seed; }
  static inline uint32_t Det(uint32_t n) {
    uint32_t m = n & 0xffff;
    uint32_t x1 = (m * 169 + 7) & 0xffff;
    uint32_t x2 = (x1 * 181 + (x1 >> 8)) & 0xffff;
    return x2;
  }
  static inline uint32_t GetWord() {
    if (det_) {
      return Det(counter_++) << 16;
    }
    rng_state_ = rng_state_ * 1664525L + 1013904223L;
    return state();
  }
  static inline int16_t GetSample() {
    return static_cast<int16_t>(GetWord() >> 16);
  }
  static inline float GetFloat() {
    return static_cast<float>(GetWord()) / 4294967296.0f;
  }
  static uint32_t rng_state_;
  static uint32_t counter_;
  static bool det_;
};

}  // namespace stmlib

#endif  // STMLIB_UTILS_RANDOM_H_
