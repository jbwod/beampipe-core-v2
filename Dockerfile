# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p beampipe-cli --bin beampipe \
    && install -D target/release/beampipe /out/beampipe

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates openssh-client \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /out/beampipe /usr/local/bin/beampipe
ENTRYPOINT ["beampipe"]
