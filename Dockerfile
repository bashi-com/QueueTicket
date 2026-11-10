FROM rust:1-bookworm AS builder
WORKDIR /app

# Cache dependencies independently of application and embedded asset changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/.fingerprint/queueticket-* \
    && rm -f target/release/queueticket target/release/deps/queueticket*

COPY src ./src
COPY templates ./templates
COPY migrations ./migrations
COPY static ./static
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system app \
    && useradd --system --gid app --no-create-home app
WORKDIR /app
# Askama templates, SQLx migrations and poll.js are embedded in the binary.
COPY --from=builder /app/target/release/queueticket /usr/local/bin/queueticket
USER app:app
ENV BIND_ADDR=0.0.0.0
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/queueticket"]
