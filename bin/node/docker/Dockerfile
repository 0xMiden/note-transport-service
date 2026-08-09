# Miden Note Transport Node
FROM rust:1.96-slim-bookworm AS builder

RUN apt-get update && \
    apt-get -y upgrade && \
    apt-get install -y llvm clang bindgen pkg-config libsqlite3-dev ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY ./Cargo.toml .
COPY ./Cargo.lock .
COPY ./bin ./bin
COPY ./crates ./crates
COPY ./proto ./proto

RUN cargo install --path bin/node --locked

FROM debian:bookworm-slim

# TARGETARCH is set by BuildKit (amd64/arm64). Plain `docker build` defaults to amd64.
ARG TARGETARCH=amd64
ARG GRPC_HEALTH_PROBE_VERSION=v0.4.54

# Update machine & install required packages
# The installation of libsqlite3-0 is needed because the binary is linked against it
# grpc_health_probe is bundled for Docker HEALTHCHECK / orchestrator probes
RUN apt-get update && \
    apt-get -y upgrade && \
    apt-get install -y --no-install-recommends \
        libsqlite3-0 \
        ca-certificates \
        curl \
    && curl -fsSL -o /usr/local/bin/grpc_health_probe \
        "https://github.com/grpc-ecosystem/grpc-health-probe/releases/download/${GRPC_HEALTH_PROBE_VERSION}/grpc_health_probe-linux-${TARGETARCH}" \
    && chmod +x /usr/local/bin/grpc_health_probe \
    && apt-get purge -y curl \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 miden \
    && useradd --uid 10001 --gid miden --shell /usr/sbin/nologin --no-create-home miden \
    && mkdir -p /db /app/data \
    && chown -R miden:miden /db /app/data

COPY --from=builder /usr/local/cargo/bin/miden-note-transport-node /usr/local/bin/miden-note-transport-node

LABEL org.opencontainers.image.authors=devops@miden.team \
    org.opencontainers.image.url=https://0xMiden.github.io/ \
    org.opencontainers.image.documentation=https://github.com/0xMiden/miden-note-transport \
    org.opencontainers.image.source=https://github.com/0xMiden/miden-note-transport \
    org.opencontainers.image.vendor=Miden \
    org.opencontainers.image.licenses=MIT

ARG CREATED
ARG VERSION
ARG COMMIT
LABEL org.opencontainers.image.created=$CREATED \
    org.opencontainers.image.version=$VERSION \
    org.opencontainers.image.revision=$COMMIT

USER miden

# Expose port
EXPOSE 57292

# gRPC health is served on the same port as the API (see tonic-health registration).
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD grpc_health_probe -addr=127.0.0.1:57292 || exit 1

CMD ["miden-note-transport-node", "--host", "0.0.0.0"]
