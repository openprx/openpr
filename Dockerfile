# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
ARG APP_BIN
WORKDIR /work

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy Cargo files
COPY Cargo.toml Cargo.toml
COPY rust-toolchain.toml rust-toolchain.toml

# Copy source code
COPY apps apps
COPY crates crates
COPY migrations migrations

# Build the specified binary
RUN cargo build --release -p ${APP_BIN}

# Runtime stage
FROM debian:bookworm-slim
ARG APP_BIN
ENV APP_BIN=${APP_BIN}
WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libpq5 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /work/target/release/${APP_BIN} /app/${APP_BIN}

# Create non-root user
RUN useradd -m -u 1000 appuser && \
    mkdir -p /app/uploads && \
    chown -R appuser:appuser /app && \
    chmod +x /app/${APP_BIN}
USER appuser

EXPOSE 8080

# Default entrypoint (can be overridden)
ENTRYPOINT ["/bin/sh", "-c"]
CMD ["exec /app/${APP_BIN}"]
