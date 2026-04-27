# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder
WORKDIR /app

# Speed up rebuilds: copy manifest first, prefetch deps, then drop in the
# real sources. `monty` is a git dependency now — Cargo fetches it directly.
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY templates ./templates
COPY static ./static
COPY src ./src

RUN cargo build --release --bin monty-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/monty-server /monty-server
COPY --from=builder /app/migrations /migrations
COPY --from=builder /app/templates /templates
COPY --from=builder /app/static /static

ENV DATA_DIR=/data
ENV ASSETS_DIR=/
EXPOSE 3000

CMD ["/monty-server"]
