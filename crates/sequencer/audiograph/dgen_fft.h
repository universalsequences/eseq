#ifndef ESEQ_DGEN_FFT_H
#define ESEQ_DGEN_FFT_H

/*
 * Portable split-complex FFT and complex multiply-accumulate for the DGen ABI
 * v1 host-services table (eseq-linux.9).
 *
 * These are drop-in replacements for the four Accelerate/vDSP calls
 * dgen_host_services.c was originally written against, and they deliberately
 * reproduce vDSP's *unscaled* conventions, because generated DGen spectral
 * code carries gain compensation tuned to them:
 *
 *   forward:  C[k] = sum(A[j] * e**(-i*2*pi*j*k/N), 0 <= j < N)
 *   inverse:  C[k] = sum(A[j] * e**(+i*2*pi*j*k/N), 0 <= j < N)
 *
 * (that is vDSP_fft_zip's documented pseudocode verbatim; neither direction
 * divides by N, so a forward/inverse round trip scales by N), and
 *
 *   accumulator = lhs * rhs + accumulator   (true complex product, no
 *                                            conjugation) — vDSP_zvma.
 *
 * Bin ordering is the natural DFT ordering, output in place, no packing: like
 * vDSP_fft_zip and unlike the real-input vDSP_fft_zrip, bin N/2 is a bin of
 * its own and is never folded into the imaginary part of bin 0.
 *
 * Like vDSP_create_fftsetup, one setup serves every transform whose length is
 * at most the length it was created for, and only setup creation allocates —
 * the transforms themselves are allocation-free and so safe to call from the
 * audio callback.
 */

#include "dgen_abi_v1.h"

/* Returns NULL if log2_size exceeds ESEQ_DGEN_FFT_MAX_LOG2_SIZE or allocation
 * fails, matching vDSP_create_fftsetup's failure return. Generated DGen code
 * NULL-checks the setup and skips the spectral op. */
DGenFFTSetupV1 eseq_dgen_portable_fft_setup_create(uint32_t log2_size);

/* Frees a setup from eseq_dgen_portable_fft_setup_create; NULL-tolerant. The
 * host-services table never calls this (setups are process-lifetime, cached in
 * statics by generated code, exactly as with vDSP); it exists for tests. */
void eseq_dgen_portable_fft_setup_destroy(DGenFFTSetupV1 setup);

/* Largest log2 length a setup may be created for. */
#define ESEQ_DGEN_FFT_MAX_LOG2_SIZE 24u

/* In-place, unscaled, split-complex. `log2_size` must be <= the setup's. */
void eseq_dgen_portable_fft_forward(
  DGenFFTSetupV1 setup, float *real, float *imaginary, uint32_t log2_size);

void eseq_dgen_portable_fft_inverse(
  DGenFFTSetupV1 setup, float *real, float *imaginary, uint32_t log2_size);

/* accumulator += lhs * rhs, elementwise complex. The accumulator may not
 * overlap lhs or rhs. */
void eseq_dgen_portable_complex_multiply_accumulate(
  const float *lhs_real,
  const float *lhs_imaginary,
  const float *rhs_real,
  const float *rhs_imaginary,
  float *accumulator_real,
  float *accumulator_imaginary,
  uint32_t element_count);

#endif
