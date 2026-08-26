/*
 * DGen ABI v1 host services.
 *
 * Port of dgen's reference implementation
 * (~/code/swift/dgen/Sources/DGenHostSupport/DGenHostSupport.c). Semantics —
 * in-place split-complex radix-2 FFT forward/inverse and split-complex
 * multiply-accumulate, including the unscaled scaling convention — must match
 * the reference exactly; generated spectral code (and its gain compensation)
 * depends on it.
 *
 * Apple platforms keep the original Accelerate/vDSP backing, byte-for-byte
 * unchanged, and this is the table the app ships there.
 *
 * Off Apple the *shipped* table is no longer this one: it is the rustfft-backed
 * table in src/lisp_host/dgen/dgen_fft.rs, because the portable kernel below
 * measured ~16x slower than rustfft and underran ALSA on spectral effects
 * (eseq-linux.78). This non-Apple branch stays compiled and pinned by the
 * host-services tests as the reference wiring of the portable kernel, and the
 * kernel itself is still linked: it is what the Rust table falls back to when
 * a setup's workspace pool is contended, and what ESEQ_DGEN_PORTABLE_FFT=1
 * forces. Both reproduce vDSP_fft_zip's and vDSP_zvma's documented conventions
 * (eseq-linux.9).
 */

#include "dgen_host_services.h"

#if defined(__APPLE__)

#include <Accelerate/Accelerate.h>

static DGenFFTSetupV1 eseq_dgen_fft_setup_create(uint32_t log2_size) {
  return (DGenFFTSetupV1)vDSP_create_fftsetup(
    (vDSP_Length)log2_size, kFFTRadix2);
}

static void eseq_dgen_fft_forward(
  DGenFFTSetupV1 setup,
  float *real,
  float *imaginary,
  uint32_t log2_size) {
  DSPSplitComplex split = {.realp = real, .imagp = imaginary};
  vDSP_fft_zip(
    (FFTSetup)setup, &split, 1, (vDSP_Length)log2_size,
    kFFTDirection_Forward);
}

static void eseq_dgen_fft_inverse(
  DGenFFTSetupV1 setup,
  float *real,
  float *imaginary,
  uint32_t log2_size) {
  DSPSplitComplex split = {.realp = real, .imagp = imaginary};
  vDSP_fft_zip(
    (FFTSetup)setup, &split, 1, (vDSP_Length)log2_size,
    kFFTDirection_Inverse);
}

static void eseq_dgen_complex_multiply_accumulate(
  const float *lhs_real,
  const float *lhs_imaginary,
  const float *rhs_real,
  const float *rhs_imaginary,
  float *accumulator_real,
  float *accumulator_imaginary,
  uint32_t element_count) {
  DSPSplitComplex lhs = {
    .realp = (float *)lhs_real, .imagp = (float *)lhs_imaginary};
  DSPSplitComplex rhs = {
    .realp = (float *)rhs_real, .imagp = (float *)rhs_imaginary};
  DSPSplitComplex accumulator = {
    .realp = accumulator_real, .imagp = accumulator_imaginary};
  vDSP_zvma(
    &lhs, 1, &rhs, 1, &accumulator, 1, &accumulator, 1,
    (vDSP_Length)element_count);
}

#else

#include "dgen_fft.h"

#define eseq_dgen_fft_setup_create eseq_dgen_portable_fft_setup_create
#define eseq_dgen_fft_forward eseq_dgen_portable_fft_forward
#define eseq_dgen_fft_inverse eseq_dgen_portable_fft_inverse
#define eseq_dgen_complex_multiply_accumulate \
  eseq_dgen_portable_complex_multiply_accumulate

#endif

static const DGenHostServicesV1 kEseqHostServicesV1 = {
  .abi_version = DGEN_ABI_VERSION_V1,
  .struct_size = sizeof(DGenHostServicesV1),
  .fft_setup_create_fn = eseq_dgen_fft_setup_create,
  .fft_forward_fn = eseq_dgen_fft_forward,
  .fft_inverse_fn = eseq_dgen_fft_inverse,
  .complex_multiply_accumulate_fn = eseq_dgen_complex_multiply_accumulate};

const DGenHostServicesV1 *eseq_dgen_host_services_v1(void) {
  return &kEseqHostServicesV1;
}
