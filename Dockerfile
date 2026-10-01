# syntax=docker/dockerfile:1

# ---- build -------------------------------------------------------------------
FROM rust:1-slim-bookworm AS build
WORKDIR /src

# Cache dependencies: build a stub first so only changed sources rebuild later.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/flicksync* target/release/flicksync* target/release/.fingerprint/flicksync*

COPY src ./src
RUN cargo build --release --locked --bin flicksync

# ---- runtime -----------------------------------------------------------------
# distroless/cc: glibc + libgcc only, no shell, no package manager, non-root variant.
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=build /src/target/release/flicksync /usr/local/bin/flicksync

ENV FLICKSYNC_HOST=0.0.0.0 \
    FLICKSYNC_PORT=8787 \
    FLICKSYNC_LOG_FORMAT=json

USER nonroot:nonroot
EXPOSE 8787

# The image has no curl/wget: the binary probes itself (honours FLICKSYNC_PORT).
HEALTHCHECK --interval=15s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/flicksync", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/flicksync"]
