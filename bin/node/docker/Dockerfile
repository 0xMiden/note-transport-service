# Miden Note Transport Node
FROM rust:1.96-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY ./Cargo.toml .
COPY ./Cargo.lock .
COPY ./bin ./bin
COPY ./crates ./crates
COPY ./proto ./proto

RUN cargo install --path bin/node --locked

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system mnt && useradd --system --gid mnt --no-create-home mnt

COPY --from=builder /usr/local/cargo/bin/miden-note-transport-node /usr/local/bin/miden-note-transport-node

LABEL org.opencontainers.image.authors=devops@miden.team \
    org.opencontainers.image.url=https://0xMiden.github.io/ \
    org.opencontainers.image.documentation=https://github.com/0xMiden/note-transport-service \
    org.opencontainers.image.source=https://github.com/0xMiden/note-transport-service \
    org.opencontainers.image.vendor=Miden \
    org.opencontainers.image.licenses=MIT

ARG CREATED
ARG VERSION
ARG COMMIT
LABEL org.opencontainers.image.created=$CREATED \
    org.opencontainers.image.version=$VERSION \
    org.opencontainers.image.revision=$COMMIT

EXPOSE 57292

USER mnt
ENTRYPOINT ["miden-note-transport-node"]
CMD ["serve", "--listen", "0.0.0.0:57292"]
