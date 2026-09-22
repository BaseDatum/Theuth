/*
** theuth: SQLCipher 4.10's "openssl" crypto provider (crypto_openssl.c, inlined in
** sqlite3.c) computes its HMACs through the OpenSSL 3 `EVP_MAC_*` / `OSSL_PARAM` API,
** which BoringSSL does not have. This header defines exactly that surface, for the
** one use the provider makes of it (HMAC over sha1/sha256/sha512), on top of
** BoringSSL's `HMAC_CTX`. Every other symbol the provider uses (EVP_CIPHER, PBKDF2,
** RAND, ERR, OBJ) exists in BoringSSL as-is.
**
** Included ahead of sqlite3.c by sqlite3_boringssl.c. Everything is `static`, so
** nothing leaks out of the sqlite3 translation unit.
*/
#ifndef THEUTH_BORINGSSL_EVP_MAC_H
#define THEUTH_BORINGSSL_EVP_MAC_H

#include <stddef.h>
#include <string.h>
#include <openssl/digest.h>
#include <openssl/hmac.h>

typedef struct {
  const char *key;
  unsigned int data_type;
  void *data;
  size_t data_size;
  size_t return_size;
} OSSL_PARAM;

#define OSSL_PARAM_UTF8_STRING 4
#define OSSL_PARAM_END { NULL, 0, NULL, 0, 0 }

typedef struct theuth_evp_mac_st EVP_MAC;
typedef HMAC_CTX EVP_MAC_CTX;

/* The provider only ever fetches "HMAC"; hand back a non-NULL token. */
static EVP_MAC *EVP_MAC_fetch(void *libctx, const char *algorithm, const char *properties) {
  static int hmac_token;
  (void)libctx; (void)properties;
  return strcmp(algorithm, "HMAC") == 0 ? (EVP_MAC *)&hmac_token : NULL;
}

static void EVP_MAC_free(EVP_MAC *mac) { (void)mac; }

static EVP_MAC_CTX *EVP_MAC_CTX_new(EVP_MAC *mac) {
  return mac ? HMAC_CTX_new() : NULL;
}

static void EVP_MAC_CTX_free(EVP_MAC_CTX *ctx) { HMAC_CTX_free(ctx); }

static int EVP_MAC_init(EVP_MAC_CTX *ctx, const unsigned char *key, size_t keylen, const OSSL_PARAM params[]) {
  const EVP_MD *md = NULL;
  const OSSL_PARAM *p;
  for (p = params; p && p->key; p++) {
    if (strcmp(p->key, "digest") != 0 || p->data_type != OSSL_PARAM_UTF8_STRING) continue;
    if (strcmp((const char *)p->data, "sha1") == 0) md = EVP_sha1();
    else if (strcmp((const char *)p->data, "sha256") == 0) md = EVP_sha256();
    else if (strcmp((const char *)p->data, "sha512") == 0) md = EVP_sha512();
  }
  if (!md) return 0;
  return HMAC_Init_ex(ctx, key, keylen, md, NULL);
}

static int EVP_MAC_update(EVP_MAC_CTX *ctx, const unsigned char *data, size_t datalen) {
  return HMAC_Update(ctx, data, datalen);
}

/* OpenSSL 3 semantics: with `out == NULL` only report the length; HMAC_Final may
** then still be called once on the same context. */
static int EVP_MAC_final(EVP_MAC_CTX *ctx, unsigned char *out, size_t *outl, size_t outsize) {
  unsigned int len;
  if (out == NULL) {
    if (outl) *outl = HMAC_size(ctx);
    return 1;
  }
  if (outsize < HMAC_size(ctx)) return 0;
  if (!HMAC_Final(ctx, out, &len)) return 0;
  if (outl) *outl = len;
  return 1;
}

#endif /* THEUTH_BORINGSSL_EVP_MAC_H */
