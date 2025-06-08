# This Dockerfile builds and packages FerriShare from a fresh clone of the repository.
# To cache both the downloads and builds of the app's dependencies it uses cargo-chef.

# Multi-stage build
# First, set up cargo-chef for proper caching of dependencies.
FROM rust:1 AS chef
# Grab and install cargo-chef from crates.io.
RUN cargo install cargo-chef
WORKDIR /app

# Now, create the `recipe.json` from the project's dependencies.
# This ensures that dependencies are only rebuilt if they change.
FROM chef AS planner
# Note that the repo's `.dockerignore` ensures several folders will not get copied over.
COPY . .
# Prepare the recipe.
RUN cargo chef prepare --recipe-path recipe.json

# Next up, download and build the dependencies.
FROM chef AS builder
# Copy over the recipe.
COPY --from=planner /app/recipe.json recipe.json
# Actually download and build the dependencies.
RUN cargo chef cook --release --recipe-path recipe.json
# Next up, the part that is not cached: Building the app itself.
COPY . .
# We set the environment variable VCS_REF here, which is read by the Rust compiler
# to insert the 10 character commit hash into the version string displayed in the app's footer.
RUN VCS_REF="$(git rev-parse --short=10 HEAD)" cargo build --release

# The app uses Tailwind CSS for its styles. This requires a build step.
FROM node:23-bookworm AS node-builder
WORKDIR /app
# Copy over package.json and package-lock.json. Should the deps change
# a redownload and rebuild will be triggered. Otherwise, they'll stay cached.
COPY ./package*.json .
# Download and install the deps.
RUN npm install
# Copy over the HTML templates, main Tailwind CSS file and configuration,
# all of which are used to generate the final CSS bundle.
COPY ./templates/ ./templates/
COPY ./main.tw.css .
COPY ./tailwind.config.js .
# Generate the stylesheet.
RUN npm run build:tw

# With all builds done, we now move to set up the container for FerriShare itself.
FROM debian:12-slim
WORKDIR /app
# Copy in the frontend templates.
COPY ./templates/ ./templates/
# Copy in the generated stylesheet
COPY --from=node-builder /app/static/main.css ./static/main.css
# Compute the hash of the generated main.css and add it to its filename.
# This way the resource can benefit from permanent browser caching.
RUN HASH_SUFFIX="$(sha256sum ./static/main.css | cut -d ' ' -f 1 | tail -c 9)" \
    && mv ./static/main.css ./static/main-${HASH_SUFFIX}.css \
    && sed -i -e "s/main.css/main-${HASH_SUFFIX}.css/g" ./templates/base.html
# Copy in the subsetted fonts ready for production use.
COPY ./font/MaterialSymbolsRounded-subset-*.woff2 ./font/
COPY ./font/InterVariable-subset-*.woff2 ./font/
# Copy in the favicon PNGs.
COPY ./favicon/*.png ./favicon/
# Copy in the compiled release binary.
COPY --from=builder /app/target/release/ferrishare .
EXPOSE 3000
ENTRYPOINT ["./ferrishare"]
