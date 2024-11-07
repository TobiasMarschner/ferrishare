// We'll use base64 to encode the key with as few characters as possible.
// To make this URL-safe we'll use `base64url`, which replaces + and / with - and _ respectively.
function base64url_encode(str) {
  return str.replaceAll('+', '-').replaceAll('/', '_');
}

function base64url_decode(str) {
  return str.replaceAll('-', '+').replaceAll('_', '/');
}

/*
  * type: One of "success", "error"
  * message: The actual text to display
*/
function updateFsStatus(type, message) {
  let fsStatus = document.getElementById("filesubmit-progress");
  let fsIcon = document.getElementById("fs-status-icon");
  let fsText = document.getElementById("fs-status-text");

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
      break;
    case 'error':
      fsIcon.textContent = 'error';
      fsStatus.classList.add('bg-red-100');
      fsIcon.classList.add('text-red-800');
      fsText.classList.add('text-red-800');
      break;
    case 'uploading':
      fsIcon.textContent = 'progress_activity';
      fsStatus.classList.add('bg-blue-100');
      fsIcon.classList.add('text-blue-800', 'animate-spin');
      fsText.classList.add('text-blue-800');
      break;
  }

  // And copy over the message.
  fsText.textContent = message;
}

async function uploadFile() {
  // Turn the status-display visible.
  document.getElementById("filesubmit-progress").style.display = "flex";

  // Grab the file selected by the user.
  let file = document.getElementById("fs-file").files[0];
  let formData = new FormData();

  if (!file) {
    updateFsStatus("error", "No file selected");
    return;
  }

  // Disable the form from here on out.
  document.getElementById("fs-expiry-fieldset").disabled = true;
  document.getElementById("fs-filebutton").disabled = true;
  document.getElementById("fs-submit").disabled = true;

  updateFsStatus("uploading", "Encrypting");

  // Extract and encode the raw file data and its filename.
  let encoder = new TextEncoder();
  let filedata = await file.arrayBuffer();
  let filename = encoder.encode(file.name);

  // Generate a random IVs for encryption. (always 96 bits)
  let iv_fd = window.crypto.getRandomValues(new Uint8Array(12));
  let iv_fn = window.crypto.getRandomValues(new Uint8Array(12));

  // Generate a random AES key to use for encryption.
  let key = await window.crypto.subtle.generateKey(
    {
      name: "AES-GCM",
      length: 256,
    },
    true,
    ["encrypt", "decrypt"],
  );

  // Encrypt the filedata and the filename.
  let e_filedata = await window.crypto.subtle.encrypt(
    {
      name: "AES-GCM",
      iv: iv_fd
    },
    key,
    filedata
  );

  let e_filename = await window.crypto.subtle.encrypt(
    {
      name: "AES-GCM",
      iv: iv_fn
    },
    key,
    filename
  );

  // // Sanity check! Decrypt:
  // let d_filedata = await window.crypto.subtle.decrypt(
  //   {
  //     name: "AES-GCM",
  //     iv: iv_fd
  //   },
  //   key,
  //   e_filedata
  // );
  // let d_filename = await window.crypto.subtle.decrypt(
  //   {
  //     name: "AES-GCM",
  //     iv: iv_fn
  //   },
  //   key,
  //   e_filename
  // );

  // let decoder = new TextDecoder();
  // let dfile = new File([d_filedata], decoder.decode(d_filename));
  // console.log(dfile);
  //
  // // OK, here we go, lol.
  // let link = document.createElement("a");
  // let url = URL.createObjectURL(dfile);
  // link.setAttribute('href', url);
  // link.setAttribute('download', dfile.name);
  // link.click();

  // Export the AES-GCM key to base64url.
  let key_b64url = base64url_encode(new Uint8Array(
    await window.crypto.subtle.exportKey("raw", key)
  ).toBase64());

  // Append the encrypted filendata and filename.
  formData.append("e_filedata", new Blob([e_filedata]));
  formData.append("e_filename", new Blob([e_filename]));

  // Append the IVs, too.
  formData.append("iv_fd", new Blob([iv_fd]));
  formData.append("iv_fn", new Blob([iv_fn]));

  // I'd love to use fetch for modern posting,
  // but if we want a regularly updating progress indicator we're stuck with XHR.
  let xhr = new XMLHttpRequest();
  xhr.open("POST", "/upload_endpoint");

  xhr.onload = () => {
    if (xhr.status == 200) {
      updateFsStatus("success", "Upload successful!");

      // Construct the download and admin links.
      const dl_link = `${location.protocol}//${location.host}/file/XXX#key=${key_b64url}`;
      const adm_link = `${location.protocol}//${location.host}/file/XXX?adm=XXX#key=${key_b64url}`;

      // Set them up in the result boxes.
      document.getElementById("fs-success-download-input").value = dl_link;
      document.getElementById("fs-success-download-link").href = dl_link;

      document.getElementById("fs-success-admin-input").value = adm_link;
      document.getElementById("fs-success-admin-link").href = adm_link;

      // And make those boxes visible.
      document.getElementById("fs-success-download-box").style.display = "flex";
      document.getElementById("fs-success-admin-box").style.display = "flex";
    } else {
      updateFsStatus("error", "Error uploading file! Status " + xhr.status);
    }
  }

  xhr.upload.onprogress = (event) => {
    let progress = (event.loaded / event.total) * 100;
    document.getElementById("fs-pbar").style.width = progress.toString() + "%";
    updateFsStatus("uploading", `Uploading ${(event.loaded / 1000000).toFixed(2)} / ${(event.total / 1000000).toFixed(2)} MB (${progress.toFixed(0)}%)`);
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

document.getElementById("fs-file").addEventListener("change", (e) => {
  if (e.target.files[0]) {
    document.getElementById("filesubmit-details").style.display = "flex";
    document.getElementById("fs-filename").textContent = e.target.files[0].name;
    document.getElementById("fs-filesize").textContent = (e.target.files[0].size / 1000000).toFixed(2) + " MB";
  }
});

