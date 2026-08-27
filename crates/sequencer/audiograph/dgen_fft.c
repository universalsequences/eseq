/*
 * Portable radix-2 split-complex FFT — see dgen_fft.h for the conventions this
 * reproduces (they are vDSP's, and generated DGen code depends on them).
 */

#include "dgen_fft.h"

#include <math.h>
#include <stdlib.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

/*
 * Twiddles are stored for the setup's maximum length only. A transform of
 * length `size` reads e**(-i*2*pi*j/len) as table[j * (max_size / len)], which
 * is exact because every length involved is a power of two — the same reason
 * one vDSP setup serves all shorter transforms.
 */
typedef struct EseqDGenFFTSetup {
  uint32_t max_log2_size;
  uint32_t max_size;
  float *twiddle_real; /* cos(-2*pi*k/max_size), k < max_size/2 */
  float *twiddle_imaginary; /* sin(-2*pi*k/max_size), k < max_size/2 */
} EseqDGenFFTSetup;

DGenFFTSetupV1 eseq_dgen_portable_fft_setup_create(uint32_t log2_size) {
  if (log2_size > ESEQ_DGEN_FFT_MAX_LOG2_SIZE) {
    return NULL;
  }

  EseqDGenFFTSetup *setup = (EseqDGenFFTSetup *)calloc(1, sizeof(*setup));
  if (setup == NULL) {
    return NULL;
  }

  setup->max_log2_size = log2_size;
  setup->max_size = 1u << log2_size;

  /* max_size == 1 has no butterflies and so no twiddles; still allocate one
   * element so the pointers are never NULL. */
  size_t half = (size_t)(setup->max_size / 2u);
  if (half == 0u) {
    half = 1u;
  }
  setup->twiddle_real = (float *)malloc(half * sizeof(float));
  setup->twiddle_imaginary = (float *)malloc(half * sizeof(float));
  if (setup->twiddle_real == NULL || setup->twiddle_imaginary == NULL) {
    eseq_dgen_portable_fft_setup_destroy((DGenFFTSetupV1)setup);
    return NULL;
  }

  /* Computed per-element in double rather than by recurrence: a recurrence
   * accumulates enough phase error at 2^24 points to show up against the
   * 2e-5 tolerance the dgen fixtures are judged at. */
  for (size_t k = 0; k < half; k++) {
    double angle = -2.0 * M_PI * (double)k / (double)setup->max_size;
    setup->twiddle_real[k] = (float)cos(angle);
    setup->twiddle_imaginary[k] = (float)sin(angle);
  }

  return (DGenFFTSetupV1)setup;
}

void eseq_dgen_portable_fft_setup_destroy(DGenFFTSetupV1 setup) {
  EseqDGenFFTSetup *typed = (EseqDGenFFTSetup *)setup;
  if (typed == NULL) {
    return;
  }
  free(typed->twiddle_real);
  free(typed->twiddle_imaginary);
  free(typed);
}

static void eseq_dgen_bit_reverse_permute(
  float *real, float *imaginary, uint32_t size) {
  uint32_t j = 0;
  for (uint32_t i = 1; i < size; i++) {
    uint32_t bit = size >> 1;
    for (; (j & bit) != 0u; bit >>= 1) {
      j ^= bit;
    }
    j ^= bit;
    if (i < j) {
      float swap = real[i];
      real[i] = real[j];
      real[j] = swap;
      swap = imaginary[i];
      imaginary[i] = imaginary[j];
      imaginary[j] = swap;
    }
  }
}

/* Iterative decimation-in-time Cooley-Tukey. `conjugate_twiddles` selects the
 * e**(+i...) kernel, i.e. the inverse direction; neither direction scales. */
static void eseq_dgen_fft_run(
  DGenFFTSetupV1 setup,
  float *real,
  float *imaginary,
  uint32_t log2_size,
  int conjugate_twiddles) {
  const EseqDGenFFTSetup *typed = (const EseqDGenFFTSetup *)setup;
  if (typed == NULL || real == NULL || imaginary == NULL) {
    return;
  }
  if (log2_size > typed->max_log2_size) {
    return;
  }

  const uint32_t size = 1u << log2_size;
  if (size < 2u) {
    return;
  }

  eseq_dgen_bit_reverse_permute(real, imaginary, size);

  for (uint32_t len = 2u; len <= size; len <<= 1) {
    const uint32_t half = len >> 1;
    const uint32_t twiddle_step = typed->max_size / len;
    for (uint32_t base = 0u; base < size; base += len) {
      for (uint32_t j = 0u; j < half; j++) {
        const uint32_t t = j * twiddle_step;
        const float twiddle_real = typed->twiddle_real[t];
        const float twiddle_imaginary =
          conjugate_twiddles ? -typed->twiddle_imaginary[t]
                             : typed->twiddle_imaginary[t];

        const uint32_t top = base + j;
        const uint32_t bottom = top + half;
        const float bottom_real = real[bottom];
        const float bottom_imaginary = imaginary[bottom];

        const float product_real =
          bottom_real * twiddle_real - bottom_imaginary * twiddle_imaginary;
        const float product_imaginary =
          bottom_real * twiddle_imaginary + bottom_imaginary * twiddle_real;

        real[bottom] = real[top] - product_real;
        imaginary[bottom] = imaginary[top] - product_imaginary;
        real[top] += product_real;
        imaginary[top] += product_imaginary;
      }
    }
  }
}

void eseq_dgen_portable_fft_forward(
  DGenFFTSetupV1 setup, float *real, float *imaginary, uint32_t log2_size) {
  eseq_dgen_fft_run(setup, real, imaginary, log2_size, 0);
}

void eseq_dgen_portable_fft_inverse(
  DGenFFTSetupV1 setup, float *real, float *imaginary, uint32_t log2_size) {
  eseq_dgen_fft_run(setup, real, imaginary, log2_size, 1);
}

void eseq_dgen_portable_complex_multiply_accumulate(
  const float *lhs_real,
  const float *lhs_imaginary,
  const float *rhs_real,
  const float *rhs_imaginary,
  float *accumulator_real,
  float *accumulator_imaginary,
  uint32_t element_count) {
  for (uint32_t i = 0u; i < element_count; i++) {
    const float product_real =
      lhs_real[i] * rhs_real[i] - lhs_imaginary[i] * rhs_imaginary[i];
    const float product_imaginary =
      lhs_real[i] * rhs_imaginary[i] + lhs_imaginary[i] * rhs_real[i];
    accumulator_real[i] += product_real;
    accumulator_imaginary[i] += product_imaginary;
  }
}
