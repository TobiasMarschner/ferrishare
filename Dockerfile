# This Dockerfile builds and packages the entire app from a fresh clone of the repository.
# To lessen the burden on bandwidth it uses cargo-chef to cache dependencies.
# If you're looking for a faster and more convenient development option,
# check out the `Dockerfile-minimal` adjacent to this one.

# Multi-stage build
# First up, set up cargo chef for proper caching of dependencies.
FROM rust:1-alpine AS chef
# We'll need `musl-dev` for successfuly builds. We might as well set it up here.
RUN apk add --no-cache musl-dev zstd
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
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
# Next up, the part that is not cached: Building the app.
COPY . .
RUN cargo build --target x86_64-unknown-linux-musl --release --bin e2ee-fileshare-rust

# Finally, set up the very minimal app-container itself.
FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/e2ee-fileshare-rust /
ENTRYPOINT ["./e2ee-fileshare-rust"]
