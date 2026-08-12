#ifndef ESEQ_DGEN_ABI_V1_H
#define ESEQ_DGEN_ABI_V1_H

/*
 * Vendored DGen generated-code runtime ABI, version 1.
 *
 * This is a faithful copy of the ABI section (context struct, host-services
 * struct, and the two export prototypes) of the dgen toolchain's
 * include/dgen_runtime.h. ESeq consumes this ABI; it does not redefine it
 * (embedded-dgen-connector-impl-spec.md, decision 6). None of the header's
 * math/intrinsics section is vendored here.
 *
 * Source: tools/dgen-toolchain/include/dgen_runtime.h
 * Source-sha256: 73baeab623bb7f2aa1f4971e1117ea3757769731f49e7d2f8967bd0a6f3a0e50
 * A Rust test (lisp_host/tests.rs, vendored_dgen_abi_header_matches_staged_
 * toolchain_header) recomputes the staged header's sha256 and fails with a
 * "re-vendor dgen_abi_v1.h" message when it drifts.
 *
 * Note (spec risk 2): dgen_set_param_value_v1 is emitted as a no-op stub by
 * design; ESeq's direct state-memory writes are the real param mechanism. If
 * a future ABI makes the export functional the two mechanisms could fight —
 * revisit at ABI v2.
 */

#include <stdint.h>

#define DGEN_ABI_VERSION_V1 1u

#ifndef DGEN_EXPORT
#define DGEN_EXPORT __attribute__((visibility("default")))
#endif

typedef void *DGenFFTSetupV1;

typedef struct DGenProcessContextV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  float sample_rate;
  uint32_t reserved;
} DGenProcessContextV1;

typedef struct DGenHostServicesV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  DGenFFTSetupV1 (*fft_setup_create_fn)(uint32_t log2_size);
  void (*fft_forward_fn)(
    DGenFFTSetupV1 setup,
    float *real,
    float *imaginary,
    uint32_t log2_size);
  void (*fft_inverse_fn)(
    DGenFFTSetupV1 setup,
    float *real,
    float *imaginary,
    uint32_t log2_size);
  void (*complex_multiply_accumulate_fn)(
    const float *lhs_real,
    const float *lhs_imaginary,
    const float *rhs_real,
    const float *rhs_imaginary,
    float *accumulator_real,
    float *accumulator_imaginary,
    uint32_t element_count);
} DGenHostServicesV1;

/*
 * Export prototypes of a compiled DGen dylib (resolved by ESeq via dlsym;
 * kept here as the documented shape of the contract, not linked directly).
 */
DGEN_EXPORT void dgen_process_v1(
  const float *const *inputs,
  float *const *outputs,
  uint32_t frame_count,
  void *state,
  const DGenProcessContextV1 *context,
  const DGenHostServicesV1 *host);

DGEN_EXPORT void dgen_set_param_value_v1(int32_t cell_id, float value);

#endif
