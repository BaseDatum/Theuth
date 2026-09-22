/*
** theuth: build the unmodified SQLCipher amalgamation against BoringSSL.
** boringssl_evp_mac.h supplies the OpenSSL-3-only EVP_MAC API the provider uses.
**
** sqlite3.c defines _GNU_SOURCE itself, but only before *its* first system include;
** the BoringSSL header pulls in libc headers first, so on Linux/glibc `mremap` and
** MREMAP_MAYMOVE would stay hidden. Define it here, before anything is included.
*/
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include "boringssl_evp_mac.h"
#include "sqlite3.c"
