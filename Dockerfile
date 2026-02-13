# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
ARG APP_BIN
WORKDIR /work

COPY Cargo.toml Cargo.toml
COPY rust-toolchain.toml rust-toolchain.toml
COPY apps apps
COPY crates crates

RUN cargo build --release -p ${APP_BIN}

FROM debian:bookworm-slim
ARG APP_BIN
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /work/target/release/${APP_BIN} /app/${APP_BIN}
EXPOSE 8080
ENTRYPOINT ["/bin/sh", "-c", "/app/${APP_BIN}"]
