document.addEventListener('DOMContentLoaded', async (event) => {
  // TEMPLATE VARIABLES
  let response_type = '{{ response_type | safe }}';
  let error_head = '{{ error_head | safe }}';
  let error_text = '{{ error_text | safe }}';
  let e_filename = new Uint8Array({{ e_filename | safe }});
  let iv_fd = new Uint8Array({{ iv_fd | safe }});
  let iv_fn = new Uint8Array({{ iv_fn | safe }});
  let filesize = {{ filesize | safe }};
  let upload_ts = '{{ upload_ts | safe }}';
  let expiry_ts = '{{ expiry_ts | safe }}';
  let views = {{ views | safe }};
  let downloads = {{ views | safe }};

  if (error_head || error_text) {
    document.getElementById('error-box-head').textContent = error_head;
    document.getElementById('error-box-text').textContent = error_text;
    document.getElementById('error-box').style.display = 'flex';
  }

  if (response_type === 'file' || response_type === 'admin') {
    document.getElementById('dl-filesize').textContent = (filesize / 1000000).toFixed(2) + " MB";
    
    // Attempt to decrypt the filename with the given key.
    // Grab the key and convert it back to binary.
    let key_string = window.location.hash.substring(5);
    let key_bytes = b64u_decBytes(key_string);

    // Construct the AES key, if possible.
    let key;

    try {
      key = await window.crypto.subtle.importKey(
        "raw",
        key_bytes,
        "AES-GCM",
        true,
        ["encrypt", "decrypt"]
      )
    } catch (e) {
      document.getElementById('error-box-head').textContent = 'Invalid key';
      document.getElementById('error-box-text').textContent = 'Cannot construct decryption key because it is corrupt or missing. This makes decrypting the file and filename impossible.';
      document.getElementById('error-box').style.display = 'flex';
      return;
    }

    // Now try decrypting the filename and put it in the document.
    try {
      let d_filename_bytes = await window.crypto.subtle.decrypt(
        {
          name: "AES-GCM",
          iv: iv_fn
        },
        key,
        e_filename
      );
      let d_filename = new TextDecoder().decode(d_filename_bytes);
      document.getElementById('dl-filename').textContent = d_filename;
    } catch (e) {
      document.getElementById('error-box-head').textContent = 'Could not decrypt filename';
      document.getElementById('error-box-text').textContent = 'Your decryption key is probably corrupt.';
      document.getElementById('error-box').style.display = 'flex';
      return;
    }

    document.getElementById('dl-box').style.display = "flex";
  }

  if (response_type === 'admin') {
    // Set all admin-only fields.
    document.getElementById('dl-upload-pretty').textContent = 'TODO';
    document.getElementById('dl-upload-ts').textContent = upload_ts;
    document.getElementById('dl-expiry-pretty').textContent = 'TODO';
    document.getElementById('dl-expiry-ts').textContent = expiry_ts;
    document.getElementById('dl-views').textContent = views;
    document.getElementById('dl-downloads').textContent = downloads;

    // Make all admin-only elements visible. (they all use flex)
    for (e of document.querySelectorAll('.admin-only')) {
      e.style.display = 'flex';
    }
  }
});
