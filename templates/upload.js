let selected_file;

async function uploadFile() {
  // Determine the expiry time.
  if (!document.querySelector("input[type='radio'][name='expires']:checked")) {
    updateInfoBox("error", "Please choose an expiry time.");
    return;
  }

  let duration = document.querySelector("input[type='radio'][name='expires']:checked").value;

  // Grab the file selected by the user.

  if (!selected_file) {
    updateInfoBox("error", "No file selected");
    return;
  }

  // Disable the form from here on out.
  document.getElementById("fs-expiry-fieldset").disabled = true;
  document.getElementById("fs-filebutton").disabled = true;
  document.getElementById("fs-submit").disabled = true;

  updateInfoBox("inprogress", "Encrypting");

  // Extract and encode the raw file data and its filename.
  let encoder = new TextEncoder();
  let filedata = await selected_file.arrayBuffer();
  let filename = encoder.encode(selected_file.name);

  let iv_filename;
  let iv_filedata;
  let e_filename;
  let e_filedata;
  let key_b64url;

  try {
    // Generate deterministic IVs (always exactly 96 bits)
    // For AES-GCM, deterministic IVs with an atomic counter are recommended over random IVs:
    // https://crypto.stackexchange.com/a/84359
    iv_filename = new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    iv_filedata = new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    // Generate a random AES key to use for encryption.
    let key = await window.crypto.subtle.generateKey(
      {
        name: "AES-GCM",
        length: 256,
      },
      true,
      ["encrypt", "decrypt"],
    );

    // Encrypt the filename.
    e_filename = await window.crypto.subtle.encrypt(
      {
        name: "AES-GCM",
        iv: iv_filename
      },
      key,
      filename
    );

    // Encrypt the filedata.
    e_filedata = await window.crypto.subtle.encrypt(
      {
        name: "AES-GCM",
        iv: iv_filedata
      },
      key,
      filedata
    );

    // Export the AES-GCM key to base64url.
    key_b64url = b64u_encBytes(new Uint8Array(
      await window.crypto.subtle.exportKey("raw", key)
    ));

  } catch (e) {
    updateInfoBox("error", "Failed to encrypt file, upload cancelled");
    console.log(e);
    return;
  }

  // Send the file over a WebSocket in chunks.
  // A single request body could exceed the size limit of a reverse proxy, such as Cloudflare's.
  const e_filedata_bytes = new Uint8Array(e_filedata);
  const chunk_size = 1024 * 1024;
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/upload_ws`);
  ws.binaryType = "arraybuffer";
  let finished = false;

  ws.onopen = () => {
    ws.send(JSON.stringify({
      e_filename: b64u_encBytes(new Uint8Array(e_filename)),
      iv_fd: b64u_encBytes(iv_filedata),
      iv_fn: b64u_encBytes(iv_filename),
      duration: duration,
      filesize: e_filedata_bytes.length,
    }));
  };

  ws.onmessage = async (event) => {
    const message = JSON.parse(event.data);
    if (message.error) {
      finished = true;
      updateInfoBox("error", message.error);
    } else if (message.ready) {
      await sendChunks(ws, e_filedata_bytes, chunk_size);
    } else {
      finished = true;
      showUploadLinks(message, key_b64url);
    }
  };

  ws.onclose = () => {
    if (!finished) {
      updateInfoBox("error", "Error during file upload");
    }
  };
}

// Send the data in chunks, keeping only a few MB queued in the browser at a time.
async function sendChunks(ws, data, chunk_size) {
  const total = data.length;
  let sent = 0;
  const reportProgress = () => {
    const done = sent - ws.bufferedAmount;
    const progress = total === 0 ? 100 : (done / total) * 100;
    document.getElementById("infobox-pbar-inner").style.width = progress.toString() + "%";
    if (done < total) {
      updateInfoBox("inprogress", `Uploading ${(done / 1000000).toFixed(2)} / ${(total / 1000000).toFixed(2)} MB (${progress.toFixed(0)}%)`);
    } else {
      updateInfoBox("inprogress", `Processing`);
    }
  };

  while (sent < total) {
    if (ws.readyState !== WebSocket.OPEN) {
      return;
    }
    if (ws.bufferedAmount > 4 * chunk_size) {
      reportProgress();
      await new Promise((resolve) => setTimeout(resolve, 50));
      continue;
    }
    const end = Math.min(sent + chunk_size, total);
    ws.send(data.subarray(sent, end));
    sent = end;
  }

  while (ws.readyState === WebSocket.OPEN && ws.bufferedAmount > 0) {
    reportProgress();
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  reportProgress();
}

function showUploadLinks(response, key_b64url) {
  // Close the normal display box and transition to the successbox.
  updateInfoBox('success', "Upload successful!");

  // Construct the download and admin links.
  const dl_link = `${location.protocol}//${location.host}/file?hash=${response.efd_sha256sum}#key=${key_b64url}`;
  const adm_link = `${location.protocol}//${location.host}/file?hash=${response.efd_sha256sum}&admin=${response.admin_key}#key=${key_b64url}`;

  // Set them up in the result boxes.
  document.getElementById("fs-success-download-input").value = dl_link;
  document.getElementById("fs-success-download-link").href = dl_link;

  document.getElementById("fs-success-admin-input").value = adm_link;
  document.getElementById("fs-success-admin-link").href = adm_link;

  // And make those boxes visible.
  document.getElementById("fs-success-download-box").style.display = "flex";
  document.getElementById("fs-success-admin-box").style.display = "flex";
}

document.getElementById("filesubmit").addEventListener("submit", (event) => {
  // We're hijacking the form's submit event.
  // Ensure the browser doesn't get any funny ideas and submits the data for us.
  event.preventDefault();
  uploadFile();
});

// Pass through click events on the "select a file" button to the actual file input that is hidden.
document.getElementById("fs-filebutton").addEventListener("click", (_) => {
  document.getElementById("fs-file").click();
});

// Process a selected file (either from the file-dialog or a drag-event).
function processFile(arg_file) {
  document.getElementById("filesubmit-details").style.display = "flex";
  document.getElementById("fs-filename").textContent = arg_file.name;
  document.getElementById("fs-filesize").textContent = (arg_file.size / 1048576).toFixed(2) + " MiB";
  // Subtract 32 from the maximum filesize here for two reasons:
  // - The WebCrypto-API appends a 16 byte authentication tag to the ciphertext.
  //   This could cause files to pass the check here but fail the size check on the backend.
  // - Near the 2GiB limit filesizes of 2GiB - 16B cause issues while 2GiB - 32B work fine.
  if (arg_file.size > max_filesize - 32) {
    document.getElementById('fs-expiry-fieldset').style.display = 'none';
    document.getElementById('fs-submit').style.display = 'none';
    updateInfoBox('error', "File too large! The maximum supported filesize is {{ max_filesize }}. Please choose a smaller file.");
  } else {
    document.getElementById('fs-expiry-fieldset').style.display = 'block';
    document.getElementById('fs-submit').style.display = 'block';
    selected_file = arg_file;
    updateInfoBox('invisible');
  }
}

document.getElementById("fs-file").addEventListener("change", (e) => {
  // Process the selected file if it exists.
  if (e.target.files[0]) {
    processFile(e.target.files[0]);
  }
});

document.querySelector("body").addEventListener("drop", (e) => {
  // Prevent the browser's default behavior, i.e don't open the file.
  e.preventDefault();

  // Don't do anything if the file-select button is disabled.
  if (document.getElementById("fs-filebutton").disabled) {
    return;
  }

  // Process the selected file if it exists.
  if (e.dataTransfer.items[0] && e.dataTransfer.items[0].kind === "file") {
    processFile(e.dataTransfer.items[0].getAsFile());
  }
});

document.querySelector("body").addEventListener("dragover", (e) => {
  // Prevent the browser's default behavior, i.e don't open the file.
  e.preventDefault();
});

document.getElementById("fs-success-download-copy").addEventListener("click", (_) => {
  let textbox = document.getElementById("fs-success-download-input");
  // Not required, but we'll select the text anyways as an indicator to the user that the operation took place.
  textbox.select();
  navigator.clipboard.writeText(textbox.value);
});

document.getElementById("fs-success-admin-copy").addEventListener("click", (_) => {
  let textbox = document.getElementById("fs-success-admin-input");
  // Not required, but we'll select the text anyways as an indicator to the user that the operation took place.
  textbox.select();
  navigator.clipboard.writeText(textbox.value);
});

// noscript handling
document.getElementById("filesubmit").style.display = 'flex';
