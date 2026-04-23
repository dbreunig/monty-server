# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder
WORKDIR /app

# Speed up rebuilds: copy manifest + monty submodule first, prefetch deps,
# then drop in the real sources.
COPY Cargo.toml Cargo.lock ./
COPY monty ./monty
COPY migrations ./migrations
COPY templates ./templates
COPY src ./src

RUN cargo build --release --bin monty-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/monty-server /monty-server
COPY --from=builder /app/migrations /migrations
COPY --from=builder /app/templates /templates

ENV DATA_DIR=/data
EXPOSE 3000

CMD ["/monty-server"]
