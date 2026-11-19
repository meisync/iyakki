/* iyakki_core.h: the C interface of the iyakki-core shared library. Made by dist/release/make-core-header.sh; do not edit. */
#ifndef IYAKKI_CORE_H
#define IYAKKI_CORE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t iyakki_core_abi_version(uint32_t *major, uint32_t *minor, uint32_t *patch);
int64_t iyakki_core_hello(uint8_t *out, size_t cap);
#ifdef __cplusplus
}
#endif
#endif
