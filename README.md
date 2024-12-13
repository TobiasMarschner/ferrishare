<p align="center">
<img width="320px" src="readme/readme_logo.png" alt="FerriShare">
</p>

<p align="center">
FerriShare is a simple, self-hostable and open-source<br>filesharing application with builtin end-to-end-encryption
</p>

## ✨ Features

- **Easily and securely share files with anyone** using a simple upload-page in your browser
    - **Files and filenames are encrypted** in your browser before being uploaded, and the key is stored in the download link's [fragment](https://en.wikipedia.org/wiki/URI_fragment) (the part after the `#`), which is never sent to the server
    - The server cannot decrypt or view the contents of the file
    - **Files automatically expire** after a chosen duration (1 hour, 1 day or 1 week)
    - Uploaders receive two links: A public download link and a private administration link
        - The latter shows download statistics and allows the uploader to delete a file early
- Builtin **IP-based rate limiting**
    - **Dual-stack support**: Uses either a full IPv4 address or a client's /64 IPv6 subnet
    - Limits the maximum number of uploads per IP (can be configured)
    - Limits the maximum number of HTTP requests per IP (can be configured)
- Configurable limits for maximum filesize and maximum storage quota
- Password-protected **site-wide administration panel**
    - shows total usage statistics and allows for early file deletion
- **Configurable Privacy Policy** (with default template) and **Legal Notice**, if you need those.
- **Fast, efficient and memory-safe backend** written entirely in **[Rust](https://www.rust-lang.org/)**, powered by [tokio](https://tokio.rs/), [axum](https://github.com/tokio-rs/axum), [tera](https://keats.github.io/tera/) and [sqlx](https://github.com/launchbadge/sqlx)
- SQLite-database for metadata storage, allowing you to deploy the entire application in a single container
- Accessible frontend with a **400 Lighthouse Score**
    - Templating is performed on the backend. JavaScript is only used when necessary.
    - Best practices: Font subsetting, permanent caching for static assets, response compression, ...

## Demo and Screenshots

<h3 align="center">You can test FerriShare on the <a href="https://ferrishare-demo.tobiasm.dev">official demo instance</a>!</h3>

#### Upload Page and Admin Link

<p>
<img height="680px" src="readme/upload_page.png" alt="Screenshot of the upload page">
<img height="680px" src="readme/admin_link.png" alt="Screenshot of an uploaded file's admin page">
</p>

#### Site-wide Administration Panel

<img height="680px" src="readme/admin_panel.png" alt="Screenshot of an uploaded file's admin page">

## 📥 Installation and Configuration

> [!WARNING]  
> While I have taken great care to correctly deploy the cryptographic primitives used in this project, I am not an expert in cryptography and this project has not been independently audited.
>
> **I cannot guarantee that the implementation or design of the system is secure.**  
> You can review the [cryptographic architectural notes](#cryptography) provided further below, or directly examine the code responsible for [encrypting](templates/upload.js) and [decrypting files](templates/download.js).
>
> If you spot any issue, please let me know in the project's issue tracker.

**FerriShare must be run behind a [reverse proxy](https://en.wikipedia.org/wiki/Reverse_proxy).**
There are two major reasons for this:

1. To encrypt files the frontend makes use of the [WebCrypto-API](https://developer.mozilla.org/en-US/docs/Web/API/Web_Crypto_API), which requires a [secure context](https://developer.mozilla.org/en-US/docs/Web/Security/Secure_Contexts).
    - This means the application must be serverd via HTTPS or on `localhost`.
2. Providing a robust and configurable TLS backend is non-trivial, and out of scope for FerriShare.

Commonly used reverse-proxies include [Traefik](https://doc.traefik.io/traefik/), [Caddy](https://caddyserver.com/docs/quick-starts/reverse-proxy) and [nginx](https://docs.nginx.com/nginx/admin-guide/web-server/reverse-proxy/).  
In the instructions presented below we will be using a very simple Traefik setup.

### With Docker (recommended)

1. Ensure both [Docker]() and [Docker Compose]() are setup and working on your machine.
    - Both rootful and [rootless](https://docs.docker.com/engine/security/rootless/) variants are supported
2. Create a folder for the application on your machine and `cd` into it.
    - For example: `mkdir ferrishare; cd ferrishare`
3. Download a copy of the repository's [`docker-compose.yml`](docker-compose.yml) into said folder
    - In it is an example setup hosting FerriShare behind Traefik on `localhost`.
      The compose-file is commented to help you better understand how to adapt it to your needs.
4. Download all of the images by invoking `docker compose pull`
5. **Configuration**: Invoke `docker compose run --rm -it ferrishare --init`
    - This will start FerriShare's interactive configuration wizard that will guide you through all options and create all necessary files in the `./data`-subdirectory.
    - You can re-run this wizard later in case you wish to reconfigure the app.
      It does not touch the database or uploaded files.
      The templates in `./data/user_templates` will only be created if they do not already exist.
6. **Launch**: Invoke `docker compose up` to launch the app in the foreground
    - Alternatively: Use `docker compose up -d` to run the containers in the background
7. **Test it out**: Use your favorite web browser to navigate to [localhost:3000](http://localhost:3000/)

### From Source

Refer to the [building locally from source](#from-source-1) instructions provided further down.

## Architectural Notes

FerriShare is built as traditional Multi-Page Application (MPA) where templating is performed fully on the backend.
In that sense there is no real separation of frontend and backend, they're intertwined.
JavaScript is only served where required, specifically the upload and download endpoints as that's where the client-side encryption takes place.

### Repository Structure

| Path | Purpose |
| ---  | ---     |
| **src/** | Rust sources for the backend |
| **templates/** | HTML and JS template sources for the frontend |
| **migrations/** | Schema files for the application's SQLite database |
| **font/** | The project's latin and icon fonts -- check the folder's [README](font) for details |
| **favicon/** | The project's favicon -- check the folder's [README](favicon) for details |
| Cargo.toml, Cargo.lock | Rust project files defining dependencies and build behavior for the backend |
| package.json, package-lock.json | npm project files used to setup the [Tailwind CLI](https://tailwindcss.com/docs/installation)
| main.tw.css, tailwind.config.js | Main stylesheet and Tailwind config used to generate the CSS bundle |
| Dockerfile | Protable build and packaging instructions (using [multi-stage builds](https://docs.docker.com/build/building/multi-stage/))
| docker-compose.yml | Example application setup with [Traefik](https://doc.traefik.io/traefik/), useful for developement or as a quick start |

### Cryptography

- Files are encyrpted with AES-GCM providing both confidentiality and integrity thanks to its AEAD nature.
- The [WebCrypto-API](https://developer.mozilla.org/en-US/docs/Web/API/Web_Crypto_API) provided by the browser is used to actually perform the en- and decryption.
    - The key is generated with `window.crypto.subtle.generateKey(...)`, which uses a strong CSPRNG.
    - IVs / nonces are randomly generated with a strong CSPRNG by using `window.crypto.getRandomValues(...)`. IVs are never reused.
- Each key is used to encrypt two messages: The filedata and the filename.
    - This generates two random IVs, putting the chance of an IV collision at 1 in 2^96. (negligible)
- The maximum safe message length with AES-GCM is 2^39 - 256 bits ≈ 64 GB.
    - This limit for the maximum filesize is enforced during configuration setup.

## 🛠️ Building Locally

### With Docker (recommended)

The instructions for building FerriShare with Docker are almost the same as the normal [installation and configuration instructions above](#with-docker-recommended), but with two main differences:
- Instead of creating an empty folder, clone this repository and `cd` into it.
- Invoke `docker compose build` instead of `docker compose pull`.
    - This causes docker compose to build the `ferrishare`-image locally from the repository sources instead of pulling them from the online registry.

The provided Dockerfile uses [multi-stage builds](https://docs.docker.com/build/building/multi-stage/) to both cache stages of the build-process and ensure the final image is as slim as possible.
It uses [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) to cache downloads and builds of all Rust dependencies,
significantly speeding up subsequent builds of the application.
The [actual Dockerfile](Dockerfile) is properly commented, check it out to understand the full build process.

### From Source

Don't like Docker? No problem.

You will need a Linux box, as all the instructions are written for a Linux machine.
MacOS and Windows have not been tested, although the former *might* work.

1. Make sure you have [Rust](https://www.rust-lang.org/tools/install) and [Node with npm](https://nodejs.org/en/download/package-manager/all) setup on your machine.
2. Clone the repository and `cd` into it
3. Install all Node dependencies by invoking `npm install`
    - This installs the [Tailwind CLI](https://tailwindcss.com/docs/installation), which is required to build the CSS bundle of the app
4. Build the CSS bundle by invoking `npm run build:tw`
    - If you prefer, you can also launch Tailwind's development server with `npm run dev:tw`
5. Build the actual application with `cargo build --release`
6. **Configuration:** Invoke `cargo run --release -- --init` (that `--` in the middle is not a typo)
    - This will start FerriShare's interactive configuration wizard that will guide you through all options and create all necessary files in the `./data`-subdirectory.
    - You can re-run this wizard later in case you wish to reconfigure the app.
      It does not touch the database or uploaded files.
      The templates in `./data/user_templates` will only be created if they do not already exist.
6. **Launch**: Invoke `cargo run --release` to launch the app in the foreground
    - **Important**: You're running and accessing the app directly without a reverse-proxy, which only works for local development.
      For this to work you must configure a `proxy-depth` of **0**, otherwise FerriShare will refuse your HTTP requests.

Note that resources served on the `/static/`-endpoint are served with an infinite cache policy.
During local development, you may want to disable browser caching to ensure your changes are always reflected in the browser.

## License and Contributing

FerriShare is released under the terms of the [MIT License](LICENSE).
Contributions are welcome!

## Code Mirrors

TODO

---

**Where does the name come from?**  
It's a simple portmanteau of 'Ferris', the Rust mascot, and 'share' from 'Fileshare'.
