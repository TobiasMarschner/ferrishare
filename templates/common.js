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

// Updating the infobox is the same for both download and upload pages.
function updateInfoBox(type, message) {
  let ib = document.getElementById("infobox");
  let ibIcon = document.getElementById("infobox-icon");
  let ibText = document.getElementById("infobox-text");
  let ibPbarOuter = document.getElementById("infobox-pbar-outer");

  // Ensure we are visible.
  ib.style.display = 'flex';

  // Clear previous coloring of the status element.
  ib.classList.remove('bg-emerald-50', 'bg-rose-50', 'bg-sky-50', 'border-emerald-500', 'border-rose-500', 'border-sky-500');
  ibIcon.classList.remove('text-emerald-700', 'text-rose-700', 'text-sky-700', 'animate-spin');
  ibText.classList.remove('text-emerald-700', 'text-rose-700', 'text-sky-700');

  // Set up colors and icon accordingly.
  switch (type) {
    case 'success':
      ibIcon.textContent = 'check_circle';
      ib.classList.add('bg-emerald-50');
      ib.classList.add('border-emerald-500');
      ibIcon.classList.add('text-emerald-700');
      ibText.classList.add('text-emerald-700');
      ibPbarOuter.style.display = "none";
      break;
    case 'error':
      ibIcon.textContent = 'error';
      ib.classList.add('bg-rose-50');
      ib.classList.add('border-rose-500');
      ibIcon.classList.add('text-rose-700');
      ibText.classList.add('text-rose-700');
      ibPbarOuter.style.display = "none";
      break;
    case 'inprogress':
      ibIcon.textContent = 'progress_activity';
      ib.classList.add('bg-sky-50');
      ib.classList.add('border-sky-500');
      ibIcon.classList.add('text-sky-700', 'animate-spin');
      ibText.classList.add('text-sky-700');
      ibPbarOuter.style.display = "flex";
      break;
  }

  // And copy over the message.
  ibText.textContent = message;
}
