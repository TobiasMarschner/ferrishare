# This Dockerfile builds and packages the entire app from a fresh clone of the repository.
# To cache the downloads and builds of the app's dependencies it uses cargo-chef.

# Multi-stage build
# First up, set up cargo chef for proper caching of dependencies.
FROM rust:1 AS chef
# Grab and install cargo-chef from crates.io.
RUN cargo install cargo-chef
WORKDIR /app

# Now, create the `recipe.json` from the project's dependencies.
# Should the resulting recipe.json change a redownload of all deps will be triggered.
FROM chef AS planner
# Note here the `.dockerignore` - several folders will not get copied over.
COPY . .
# Prepare the recipe with all deps.
RUN cargo chef prepare --recipe-path recipe.json

# Next up, actually download and cache the deps.
FROM chef AS builder
# Copy over the recipe. If it stayed the same no redownload of deps will take place.
COPY --from=planner /app/recipe.json recipe.json
# Actually build deps.
RUN cargo chef cook --release --recipe-path recipe.json
# Next up, the part that is not cached: Building the app itself.
COPY . .
RUN cargo build --release

# Build the Tailwind CSS using node.
FROM node AS node-builder
WORKDIR /app
# Copy over package.json and package-lock.json. Should the deps change
# a redownload and rebuild will be triggered. Otherwise they'll stay cached.
COPY ./package*.json .
# Download and install the deps.
RUN npm install
# Copy over the templates used to actually generate the styles.
COPY ./templates/ ./templates/
COPY ./main.tw.css .
COPY ./tailwind.config.js .
# Actually generate the stylesheet.
RUN npm run build:tw

# Finally, set up the very minimal app-container itself.
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
# Copy in those font-files that we actually use in production.
COPY ./font/MaterialSymbolsRounded-subset-*.woff2 ./font/
COPY ./font/InterVariable-subset-*.woff2 ./font/
# Copy in the compiled release binary.
COPY --from=builder /app/target/release/FerriShare .
EXPOSE 3000
ENTRYPOINT ["./FerriShare"]
