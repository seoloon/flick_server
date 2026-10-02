# syntax=docker/dockerfile:1
#
# One file, two images. Pick the one you want with --target:
#
#   docker build --target flicksync -t flicksync .   # the sync service (default: the last stage)
#   docker build --target panel     -t flick-panel . # the optional web panel
#
# Each target builds only its own stages, so the Rust and Node toolchains never meet.

# ============================================================================
# panel: Next.js web panel (node:22-alpine)
# ============================================================================
FROM node:22-alpine AS panel-deps
WORKDIR /app
COPY panel/package.json panel/package-lock.json ./
RUN npm ci

FROM node:22-alpine AS panel-build
WORKDIR /app
COPY --from=panel-deps /app/node_modules ./node_modules
COPY panel/ ./
ENV NEXT_TELEMETRY_DISABLED=1 NEXT_OUTPUT=standalone
RUN npm run build

FROM node:22-alpine AS panel
WORKDIR /app
ENV NODE_ENV=production NEXT_TELEMETRY_DISABLED=1 PORT=3000 HOSTNAME=0.0.0.0
COPY --from=panel-build /app/.next/standalone ./
COPY --from=panel-build /app/.next/static ./.next/static
COPY --from=panel-build /app/public ./public
COPY --from=panel-build /app/start.mjs ./start.mjs
USER node
EXPOSE 3000
# No curl in the image: probe the login page with node itself.
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD node -e "fetch('http://127.0.0.1:'+(process.env.PORT||3000)+'/login').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
CMD ["node", "start.mjs"]

# ============================================================================
# flicksync: the sync service (Rust, distroless). Last stage = default target.
# ============================================================================
FROM rust:1-slim-bookworm AS flicksync-build
WORKDIR /src

# Cache dependencies: build a stub first so only changed sources rebuild later.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/flicksync* target/release/flicksync* target/release/.fingerprint/flicksync*

COPY src ./src
RUN cargo build --release --locked --bin flicksync \
    && mkdir /data

# distroless/cc: glibc + libgcc only, no shell, no package manager, non-root variant.
FROM gcr.io/distroless/cc-debian12:nonroot AS flicksync

COPY --from=flicksync-build /src/target/release/flicksync /usr/local/bin/flicksync
# Owned by nonroot so a fresh named volume mounted here is writable (holds the signing key).
COPY --from=flicksync-build --chown=nonroot:nonroot /data /data

ENV FLICKSYNC_HOST=0.0.0.0 \
    FLICKSYNC_PORT=8787 \
    FLICKSYNC_LOG_FORMAT=json \
    FLICKSYNC_DATA_DIR=/data

USER nonroot:nonroot
EXPOSE 8787

# The image has no curl/wget: the binary probes itself (honours FLICKSYNC_PORT).
HEALTHCHECK --interval=15s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/flicksync", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/flicksync"]
