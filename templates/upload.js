let selected_file;

async function uploadFile() {
  // Determine the expiry time.
  if (!document.querySelector("input[type='radio'][name='expires']:checked")) {
    updateInfoBox("error", "Please choose an expiry time.");
    return;
  }

  let duration = document.querySelector("input[type='radio'][name='expires']:checked").value;

  // Grab the file selected by the user.
  let formData = new FormData();

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

  let iv_fn;
  let iv_fd;
  let e_filename;
  let e_filedata;
  let key_b64url;

  try {
    // Generate random IVs for encryption. (always exactly 96 bits)
    iv_fn = window.crypto.getRandomValues(new Uint8Array(12));
    iv_fd = window.crypto.getRandomValues(new Uint8Array(12));

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
        iv: iv_fn
      },
      key,
      filename
    );

    // Encrypt the filedata.
    e_filedata = await window.crypto.subtle.encrypt(
      {
        name: "AES-GCM",
        iv: iv_fd
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

  // Append all the data that's supposed to go to the server.
  formData.append("e_filedata", new Blob([e_filedata]));
  formData.append("e_filename", new Blob([e_filename]));
  formData.append("iv_fd", new Blob([iv_fd]));
  formData.append("iv_fn", new Blob([iv_fn]));
  formData.append("duration", duration);

  // I'd love to use fetch for modern posting,
  // but if we want a regularly updating progress indicator we're stuck with XHR.
  let xhr = new XMLHttpRequest();
  xhr.open("POST", "/upload_endpoint");

  xhr.onerror = () => {
    updateInfoBox("error", "Error during file upload")
  }

  xhr.onload = () => {
    if (xhr.status == 201) {
      // Close the normal display box and transition to the successbox.
      // document.getElementById("infobox").style.display = 'none';
      // document.getElementById("successbox").style.display = 'flex';
      updateInfoBox('success', "Upload successful!");

      let response = JSON.parse(xhr.response);

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
    } else {
      updateInfoBox("error", xhr.responseText);
    }
  }

  xhr.upload.onprogress = (event) => {
    let progress = (event.loaded / event.total) * 100;
    document.getElementById("infobox-pbar-inner").style.width = progress.toString() + "%";
    if (event.loaded < event.total) {
      updateInfoBox("inprogress", `Uploading ${(event.loaded / 1000000).toFixed(2)} / ${(event.total / 1000000).toFixed(2)} MB (${progress.toFixed(0)}%)`);
    } else {
      updateInfoBox("inprogress", `Processing`);
    }
  }

  xhr.send(formData);
}

document.getElementById("filesubmit").addEventListener("submit", (event) => {
  // We're hijacking the form's submit event.
  // Ensure the browser doesn't get any funny ideas and submits the data for us.
  event.preventDefault();
  uploadFile();
});

// Pass through click events on the "select a file" button to the actual file input that is hidden.
document.getElementById("fs-filebutton").addEventListener("click", (event) => {
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
