# FerriShare

**FerriShare** --- a simple, self-hostable filesharing application with builtin end-to-end-encryption

- 🔒 Files and filenames are encrypted in your browser before being sent over the network
    - 🕶️ The server cannot decrypt or view the file
    - 🔑 The decryption key is stored in the download link's [fragment]() (the part after the `#`), which is never sent to the server
    - 🗑️ Files automatically expire after a chosen duration
- 🌐 On upload two links are created:
    - A public download link showing filename and filesize
    - A private administration link with download statistics, allowing early file deletion
- 📊 Site-wide administration panel with total usage statistics and the ability to delete files early
- 🛡️ Builtin and configurable IP-based rate limiting
    - Dual-stack support: Limits either by IPv4 address or by /64 IPv6 subnet
    - Limit the maximum number of uploads per IP
    - Limit the maximum number of HTTP requests per IP
- 📃 Configurable Privacy Policy (with default template) and Legal Notice, if you need those
- 🚀 Fast, efficient and memory-safe
    - 🦀 Backend entirely written in Rust, powered by tokio, axum, tera and sqlx
    - 
    - small bundles, subsetting
    - Rust backend, efficient, sqlite db, portable, single container TODO

## ✨ Demo and Screenshots

Test out the demo for yourself at [ferrishare-demo.tobiasm.dev](https://ferrishare-demo.tobiasm.dev)! Uploads automatically expire after 15 minutes and you can login to the admin interface with password "admin".

## 📥 Installation and Configuration

> [!WARNING]  
> While I have taken great care to correctly deploy the cryptographic primitives used in this project,  
> I am not an expert in cryptography and this project has not been independently audited.
>
> **I cannot guarantee that the implementation or design of the system is secure.**  
> You can review [cryptographic architectural notes](#🗝%EF%B8%8F-cryptography) provided further below,  
> or directly examine the code responsible for [encrypting files](templates/upload.js) or [decrypting files](templates/download.js).
>
> If you spot any issue, please let me know in the project's issue tracker.

**FerriShare must be run behind a [reverse proxy](https://en.wikipedia.org/wiki/Reverse_proxy).**
There are two major reasons for this:

1. To encrypt files the frontend makes use of the [WebCrypto-API](https://developer.mozilla.org/en-US/docs/Web/API/Web_Crypto_API), which requires a [secure context](https://developer.mozilla.org/en-US/docs/Web/Security/Secure_Contexts).
    - This means the application must be serverd via HTTPS or on `localhost`.
2. Providing a robust and configurable TLS backend is non-trivial, and out of scope for FerriShare.

Commonly used reverse-proxies include [Traefik](https://doc.traefik.io/traefik/), [Caddy](https://caddyserver.com/docs/quick-starts/reverse-proxy) and [nginx](https://docs.nginx.com/nginx/admin-guide/web-server/reverse-proxy/).  
In the instructions presented below we will be using a very simple Traefik setup.

### 🐳 With Docker (recommended)

1. Ensure both [Docker]() and [Docker Compose]() are setup and working on your machine.
    - Both rootful and [rootless](https://docs.docker.com/engine/security/rootless/) variants are supported
2. Create a folder for the application on your machine and `cd` into it.
    - For example: `mkdir ferrishare; cd ferrishare`
3. Download a copy of the repository's [`docker-compose.yml`](/docker-compose.yml) into said folder
4. Download all of the images by invoking `docker compose pull`
5. **Configuration**: Invoke `docker compose run --rm -it ferrishare --init`
    - This will start FerriShare's interactive configuration wizard that will guide you through all options and create all necessary files in the `./data`-subdirectory.
    - You can re-run this wizard later in case you wish to reconfigure the app.
      It does not touch the database or uploaded files.
      The templates in `./data/user_templates` will only be created if they do not already exist.
6. **Launch**: Invoke `docker compose up` to launch the app in the foreground
    - Alternatively: Use `docker compose up -d` to run the containers in the background
7. **Test it out**: Use your favorite web browser to navigate to [localhost:3000](http://localhost:3000/)

### 📝 From Source

Refer to the [building locally from source](#📝-from-source-2) instructions provided further down.

## 📐 Architectural Notes

### 🗝️ Cryptography

#### File encryption

- Files are encyrpted with AES-GCM providing both confidentiality and integrity thanks to its AEAD nature.
- The [WebCrypto-API]() provided by the browser is used to actually perform the en- and decryption.
    - The key is generated with ``, which uses a strong CSPRNG.
    - IVs / nonces are randomly generated using strong CSPRNGS. IVs are never reused.
- Each key is used to encrypt two messages: The filedata and the filename.
    - This generates two random IVs, putting the chance of an IV collision at 1 in 2^96. (negligible)
- The maximum safe message length with AES-GCM is 2^39 - 256 bits ≈ 64 GB.
    - This limit for the maximum filesize is enforced during configuration setup.

#### Backend

### 📁 Repository Structure

## 🛠️ Building Locally

Want to hack on FerriShare yourself? Make changes just for your deployment? No problem.

### 🐳 With Docker (recommended)

The instructions for building FerriShare with Docker are almost the same as the normal [installation and configuration instructions above](#🐳-with-docker-(recommended)), but with two main differences:
- Instead of creating an empty folder, clone this repository and `cd` into it.
- Invoke `docker compose build` instead of `docker compose pull`.
    - This causes docker compose to build the `ferrishare`-image locally from the repository sources instead of pulling them from the online registry.

The provided Dockerfile uses [multi-stage builds](https://docs.docker.com/build/building/multi-stage/) to both cache stages of the build-process and ensure the final image is as slim as possible.
It uses [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) to cache downloads and builds of all Rust dependencies,
significantly speeding up subsequent builds of the application.
The [actual Dockerfile](/Dockerfile) is properly commented, check it out to understand the full build process.

### 📝 From Source

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
    - **Important**: You're running and accessing the app directly without a reverse-proxy, which only works for local developement.
      For this to work you must configure a `proxy-depth` of **0**, otherwise FerriShare will refuse your HTTP requests.

Note that resources served on the `/static/`-endpoint are served with an infinite cache policy.
During local development, you may want to disable browser caching to ensure your changes are always reflected in the browser.

---

**Where does the name come from?**  
It's a simple portmanteau of 'Ferris', the Rust mascot, and 'share' from 'Fileshare'.
