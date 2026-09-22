/*
** theuth: build the unmodified SQLCipher amalgamation against BoringSSL.
** boringssl_evp_mac.h supplies the OpenSSL-3-only EVP_MAC API the provider uses.
*/
#include "boringssl_evp_mac.h"
#include "sqlite3.c"
