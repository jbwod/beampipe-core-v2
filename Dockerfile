# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
ARG TARGETPLATFORM
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY assets/brand/beampipe-terminal-logo.txt ./assets/brand/beampipe-terminal-logo.txt
# Operator sample files are compiled from crates/beampipe-cli/embedded via include_str!.

# Scope caches by platform. amd64 and arm64 otherwise race on
# registry/src/<crate>/.cargo-ok (EEXIST) during a multi-arch buildx bake.
RUN --mount=type=cache,id=cargo-registry-${TARGETPLATFORM},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git-${TARGETPLATFORM},target=/usr/local/cargo/git \
    --mount=type=cache,id=cargo-target-${TARGETPLATFORM},target=/app/target \
    cargo build --locked --release -p beampipe-cli --bin beampipe \
    && strip target/release/beampipe \
    && install -D target/release/beampipe /out/beampipe

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 beampipe \
    && useradd --uid 10001 --gid beampipe --create-home --home-dir /var/lib/beampipe beampipe
COPY --from=builder /out/beampipe /usr/local/bin/beampipe
WORKDIR /var/lib/beampipe
USER 10001:10001
ENTRYPOINT ["beampipe"]
