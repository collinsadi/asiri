# syntax=docker/dockerfile:1
# Builds asiri-relay from this repository's source, on the server (architecture §15.4).
ARG RUST_IMAGE=rust:1.97-slim-trixie
ARG RUNTIME_IMAGE=debian:trixie-slim

FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY . .
# Use the image's toolchain instead of downloading the exact pin in rust-toolchain.toml. The
# crates and the build directory are cache mounts: an update recompiles only what changed, and
# old builds don't pile up on the server's disk.
RUN --mount=type=cache,id=asiri-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=asiri-relay-target,target=/src/target \
    rm -f rust-toolchain.toml \
 && cargo build --release --locked -p asiri-relay \
 && strip target/release/asiri-relay \
 && cp target/release/asiri-relay /usr/local/bin/asiri-relay

FROM ${RUNTIME_IMAGE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/asiri-relay /usr/local/bin/asiri-relay
USER 10001:10010
ENV ASIRI_DATA_DIR=/var/lib/asiri \
    ASIRI_AUTHORIZED_DIR=/var/lib/asiri-authorized \
    ASIRI_LISTEN=127.0.0.1:8080 \
    ASIRI_LOG=warn
ENTRYPOINT ["/usr/local/bin/asiri-relay"]
CMD ["serve"]
