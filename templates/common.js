// Suite of utilities for working with base64 in the browser.
// Specifically, we will be working with base64url,
// which replaces + and / with - and _ to make the strings URL-safe.
//
// The methods are adapted from the excellent MDN resource on the topic:
// https://developer.mozilla.org/en-US/docs/Web/API/Window/btoa

// Encode Uint8Array to base64url-string, and truncate the padding.
function b64u_encBytes(bytes) {
  const binString = Array.from(bytes, (byte) =>
    String.fromCodePoint(byte),
  ).join("");
  return btoa(binString).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')
                                                                   
}

// Decode base64url-string to Uint8Array.
function b64u_decBytes(base64) {
  const binString = atob(base64.replaceAll('-', '+').replaceAll('_', '/'));
  return Uint8Array.from(binString, (m) => m.codePointAt(0));
}

// Encode JS-string to base64url-string (via Uint8Array and TextEncoder) and truncate the padding.
function b64u_encString(str) {
  return b64u_encBytes(new TextEncoder().encode(str));
}

// Decode base64url-string to JS-string. (via Uint8Array and TextEncoder)
function b64u_decString(base64) {
  return new TextEncoder().decode(b64u_decBytes(base64));
}
