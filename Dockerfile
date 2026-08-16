# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY deploy/operator/docker-compose.yml deploy/operator/.env.example ./deploy/operator/
COPY config/wallaby_hires.v2.yaml ./config/wallaby_hires.v2.yaml

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
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
