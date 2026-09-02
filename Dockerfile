# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
ARG APP_BIN
ARG OPENPR_BUILD_GIT_COMMIT=unknown
ARG OPENPR_BUILD_GIT_DIRTY=unknown
ARG OPENPR_BUILD_GIT_COMMITTER_DATE=unknown
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

# The Docker build context excludes .git. Release automation must pass all three
# provenance args; omitted metadata stays explicit "unknown" and is rejected by
# the deployed provenance gate rather than being mistaken for a clean build.
RUN OPENPR_BUILD_GIT_COMMIT="${OPENPR_BUILD_GIT_COMMIT}" \
    OPENPR_BUILD_GIT_DIRTY="${OPENPR_BUILD_GIT_DIRTY}" \
    OPENPR_BUILD_GIT_COMMITTER_DATE="${OPENPR_BUILD_GIT_COMMITTER_DATE}" \
    cargo build --release -p ${APP_BIN}

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
