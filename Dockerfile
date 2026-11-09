# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder
WORKDIR /app

# Sandboxed code runs in `monty` worker subprocesses. Build the worker from
# the `monty-runtime` crate, pinned to the same version as `monty-pool` in
# Cargo.toml (the wire protocol must match). `--no-default-features` drops the
# interactive CLI; the pool only needs `monty subprocess`. This layer only
# changes when the version does, so it caches across app rebuilds.
ARG MONTY_VERSION=1.5.0
RUN cargo install monty-runtime --version ${MONTY_VERSION} --no-default-features --locked --root /usr/local

COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY templates ./templates
COPY static ./static
COPY src ./src

RUN cargo build --release --bin monty-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/monty-server /monty-server
# Found automatically as a sibling of the server binary.
COPY --from=builder /usr/local/bin/monty /monty
COPY --from=builder /app/migrations /migrations
COPY --from=builder /app/templates /templates
COPY --from=builder /app/static /static

ENV DATA_DIR=/data
ENV ASSETS_DIR=/
EXPOSE 3000

CMD ["/monty-server"]
