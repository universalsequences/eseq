// RREF_U=<u> -> constant GetFloat(); RREF_SEED=<n> -> LCG seed.
#include <cstdio>
#include <cstdlib>
#include "stmlib/utils/random.h"
namespace stmlib {
uint32_t Random::rng_state_ = getenv("RREF_SEED") ? uint32_t(atol(getenv("RREF_SEED"))) : 0x21;
float Random::const_u_ = getenv("RREF_U") ? float(atof(getenv("RREF_U"))) : -1.0f;
size_t Random::sample_base_ = 0;
size_t Random::noise_size_ = 0;
static const float* load_noise() {
  const char* path = getenv("RREF_NOISE");
  if (!path) return nullptr;
  FILE* f = fopen(path, "rb");
  if (!f) return nullptr;
  fseek(f, 0, SEEK_END);
  long bytes = ftell(f);
  fseek(f, 0, SEEK_SET);
  float* data = static_cast<float*>(malloc(bytes));
  fread(data, 1, bytes, f);
  fclose(f);
  Random::noise_size_ = size_t(bytes) / (2 * sizeof(float));
  return data;
}
const float* Random::noise_ = load_noise();
}
