#ifndef ESEQ_DGEN_HOST_SERVICES_H
#define ESEQ_DGEN_HOST_SERVICES_H

/*
 * ESeq's implementation of the DGen ABI v1 host-services table
 * (embedded-dgen-connector-impl-spec.md, decision 2 / slice E4): a
 * near-verbatim port of dgen's reference Sources/DGenHostSupport/
 * DGenHostSupport.c. Backed by Accelerate/vDSP on Apple platforms — where the
 * *app* links Accelerate and generated dylibs never do — and by the portable
 * dgen_fft.c everywhere else (eseq-linux.9).
 *
 * Off Apple this table is no longer the one the app installs; see the note in
 * dgen_host_services.c (eseq-linux.78).
 */

#include "dgen_abi_v1.h"

/* Returns process-lifetime statics; never NULL. */
const DGenHostServicesV1 *eseq_dgen_host_services_v1(void);

/*
 * The portable C table is exposed separately on every platform for explicit
 * fallback selection and end-to-end conformance tests. On non-Apple targets
 * this is distinct from the shipped rustfft-backed table owned by Rust.
 */
const DGenHostServicesV1 *eseq_dgen_portable_host_services_v1(void);

#endif
