// Standard base64 over the WebView's `atob`/`btoa`. Not `Uint8Array.toBase64`/`fromBase64`: WebKit's minimum
// versions here predate them.

/** Standard base64 (with padding, no `data:` prefix) of `bytes`. */
export function encode(bytes: Uint8Array): string {
  let s = ''
  // Chunks keep `fromCharCode`'s argument list under the engines' limits (a multi-MiB spread overflows it).
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

/** Bytes of standard base64 in their own `ArrayBuffer`; throws (`atob`'s `InvalidCharacterError`) on bad input. */
export function decode(b64: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))
}

/** Base64 of a string's UTF-8 bytes. */
export const utf8ToB64 = (s: string): string => encode(new TextEncoder().encode(s))

/** A string from base64 of UTF-8 bytes; throws on bad base64. */
export const b64ToUtf8 = (b64: string): string => new TextDecoder().decode(decode(b64))
