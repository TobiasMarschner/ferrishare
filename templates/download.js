document.addEventListener('DOMContentLoaded', async (event) => {
  if (error_head || error_text) {
    document.getElementById('error-box-head').textContent = error_head;
    document.getElementById('error-box-text').textContent = error_text;
    document.getElementById('error-box').style.display = 'flex';
  }

  if (response_type === 'file' || response_type === 'admin') {
    document.getElementById('dl-filesize').textContent = (filesize / 1000000).toFixed(2) + " MB";

    // Attempt to decrypt the filename with the given key.
    // Grab the key and convert it back to binary.
    let key_bytes = b64u_decBytes(key_string);

    // Construct the AES key, if possible.
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
      d_filename = new TextDecoder().decode(d_filename_bytes);
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
    document.getElementById('dl-upload-pretty').textContent = upload_ts_pretty;
    document.getElementById('dl-upload-ts').textContent = upload_ts;
    document.getElementById('dl-expiry-pretty').textContent = expiry_ts_pretty;
    document.getElementById('dl-expiry-ts').textContent = expiry_ts;
    document.getElementById('dl-downloads').textContent = downloads;

    // Compute the "normal" download-link-box and set it up.
    const dl_link = `${location.protocol}//${location.host}/file?hash=${efd_sha256sum}#key=${key_string}`;
    document.getElementById("admin-download-input").value = dl_link;
    document.getElementById("admin-download-link").href = dl_link;

    // Make all admin-only elements visible. (they all use flex)
    for (e of document.querySelectorAll('.admin-only')) {
      e.style.display = 'flex';
    }
  }
});

function updateDlStatus(type, message) {
  let fsStatus = document.getElementById("download-progress");
  let fsIcon = document.getElementById("dl-status-icon");
  let fsText = document.getElementById("dl-status-text");
  let fsPbar = document.getElementById("dl-pbar");

  // Ensure the box itself is always visible.
  fsStatus.style.display = 'flex';

  // Clear previous coloring of the status element.
  fsStatus.classList.remove('bg-gray-200', 'bg-green-100', 'bg-red-100', 'bg-blue-100');
  fsIcon.classList.remove('text-gray-800', 'text-green-800', 'text-red-800', 'text-blue-800', 'animate-spin');
  fsText.classList.remove('text-gray-800', 'text-green-800', 'text-red-800', 'text-blue-800');

  // Set up colors and icon accordingly.
  switch (type) {
    case 'success':
      fsIcon.textContent = 'check_circle';
      fsStatus.classList.add('bg-green-100');
      fsIcon.classList.add('text-green-800');
      fsText.classList.add('text-green-800');
      fsPbar.style.display = "none";
      break;
    case 'error':
      fsIcon.textContent = 'error';
      fsStatus.classList.add('bg-red-100');
      fsIcon.classList.add('text-red-800');
      fsText.classList.add('text-red-800');
      fsPbar.style.display = "none";
      break;
    case 'inprogress':
      fsIcon.textContent = 'progress_activity';
      fsStatus.classList.add('bg-blue-100');
      fsIcon.classList.add('text-blue-800', 'animate-spin');
      fsText.classList.add('text-blue-800');
      fsPbar.style.display = "flex";
      break;
  }

  // And copy over the message.
  fsText.textContent = message;
}

// Set up the handler for the actual download button.
document.getElementById("download-button").addEventListener("click", (_) => {
  // I'd love to use fetch for modern posting,
  // but if we want a regularly updating progress indicator we're stuck with XHR.
  let xhr = new XMLHttpRequest();
  xhr.open("GET", `/download_endpoint?hash=${efd_sha256sum}`);
  // Immediately store the response into an arraybuffer.
  xhr.responseType = 'arraybuffer';

  let dlbutton = document.getElementById("download-button");
  let dlprogress = document.getElementById("download-progress");
  let dlprog_pbar_inner = document.getElementById("dl-pbar-inner");

  xhr.onload = async () => {
    if (xhr.status == 200) {
      try {
        updateDlStatus('inprogress', "Decrypting");

        // Now actually decrypt the file.
        d_filedata = await window.crypto.subtle.decrypt(
          {
            name: "AES-GCM",
            iv: iv_fd
          },
          key,
          xhr.response
        );

        // Assemble the file.
        let d_file = new File([d_filedata], d_filename);

        // And download it.
        let link = document.createElement("a");
        let url = URL.createObjectURL(d_file);
        link.setAttribute('href', url);
        link.setAttribute('download', d_file.name);
        link.click();
      } catch (e) {
        console.log(e);
        updateDlStatus("error", "Could not decrypt file");
      }

      updateDlStatus('success', "File downloaded");
    } else {
      updateDlStatus("error", "Error during file download");
    }
  }

  xhr.onprogress = (event) => {
    let progress = (event.loaded / filesize) * 100;
    dlprog_pbar_inner.style.width = progress.toString() + "%";
    updateDlStatus("inprogress", `Downloading ${(event.loaded / 1000000).toFixed(2)} / ${(filesize / 1000000).toFixed(2)} MB (${progress.toFixed(0)}%)`);
  }

  // Disable download button while the operation is ongoing.
  dlbutton.disabled = true;
  xhr.send();
});

document.getElementById("admin-download-copy").addEventListener("click", (_) => {
  let textbox = document.getElementById("admin-download-input");
  // Not required, but we'll select the text anyways as an indicator to the user that the operation took place.
  textbox.select();
  navigator.clipboard.writeText(textbox.value);
});

document.getElementById("delete-button").addEventListener("click", () => {
  let xhr = new XMLHttpRequest();

  xhr.open('POST', '/delete_endpoint');
  xhr.setRequestHeader('Content-Type', 'application/json');

  xhr.onload = () => {
    if (xhr.status === 200) {
      document.getElementById('error-box-head').textContent = 'The file has been deleted';
      document.getElementById('error-box-icon').textContent = 'check_circle';
      document.getElementById('error-box-text').style.display = 'none';
      document.getElementById('error-box').style.display = 'flex';
      document.getElementById('dl-box').style.display = 'none';
    } else {
      updateDlStatus('error', 'Error during deletion request')
      document.getElementById('download-button').disabled = false;
      document.getElementById('delete-button').disabled = false;
    }
  }

  document.getElementById('download-button').disabled = true;
  document.getElementById('delete-button').disabled = true;

  xhr.send(JSON.stringify({
    hash: efd_sha256sum,
    admin: admin_key
  }))
});
