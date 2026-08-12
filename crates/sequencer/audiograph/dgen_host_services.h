#ifndef ESEQ_DGEN_HOST_SERVICES_H
#define ESEQ_DGEN_HOST_SERVICES_H

/*
 * ESeq's implementation of the DGen ABI v1 host-services table
 * (embedded-dgen-connector-impl-spec.md, decision 2 / slice E4): a
 * near-verbatim port of dgen's reference Sources/DGenHostSupport/
 * DGenHostSupport.c, backed by Accelerate/vDSP. The *app* links Accelerate;
 * generated dylibs never do.
 */

#include "dgen_abi_v1.h"

/* Returns a process-lifetime static; never NULL. */
const DGenHostServicesV1 *eseq_dgen_host_services_v1(void);

#endif
