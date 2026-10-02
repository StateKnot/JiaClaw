# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
ARG RUST_IMAGE=rust:1.85.0-bookworm@sha256:0ff31c9ffa641a62e48d543fb00b4960955ea375f40776f40f585b89e654cc5e
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
FROM ${RUST_IMAGE} AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked -p jiaclaw-host

FROM ${RUNTIME_IMAGE} AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 jiaclaw \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data jiaclaw \
    && mkdir -p /data/workspace /data/state \
    && chown -R 10001:10001 /data
COPY --from=builder /build/target/release/jiaclaw /usr/local/bin/jiaclaw
USER 10001:10001
ENV HOME=/data
WORKDIR /data
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/jiaclaw"]
CMD ["serve", "--config", "/etc/jiaclaw/config.toml"]
