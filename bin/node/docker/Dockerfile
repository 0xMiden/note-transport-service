# Miden Note Transport Node
FROM rust:1.90-slim-bookworm as builder



RUN apt-get update && \
    apt-get -y upgrade && \
    apt-get install -y llvm clang bindgen pkg-config libssl-dev libsqlite3-dev ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY ./Cargo.toml .
COPY ./Cargo.lock .
COPY ./bin ./bin
COPY ./crates ./crates
COPY ./proto ./proto

RUN cargo install --path bin/node --locked

FROM debian:bookworm-slim

# Update machine & install required packages
# The installation of sqlite3 is needed for correct function of the SQLite database
RUN apt-get update && \
    apt-get -y upgrade && \
    apt-get install -y --no-install-recommends \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/miden-note-transport-node-bin /usr/local/bin/miden-note-transport-node-bin

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

# Expose port
EXPOSE 57292

CMD ["miden-note-transport-node-bin", "--host", "0.0.0.0"]
