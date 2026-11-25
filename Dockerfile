# syntax=docker/dockerfile:1
# Multi-arch image for linux/amd64 and linux/arm64. Both build stages run on the build machine's own
# architecture: Rust cross-compiles, so no stage is emulated with QEMU.

# ---- GUI ------------------------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM node:24-bookworm-slim AS web
RUN npm install --global pnpm@12.22.0
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

# ---- Server ---------------------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM rust:1.96-bookworm AS server
ARG TARGETARCH
# A C cross compiler for the bundled SQLite and ring, and the Rust target to match.
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu > /rust-target; echo x86-64-linux-gnu > /gcc-pkg; echo x86_64-linux-gnu-gcc > /gcc ;; \
      arm64) echo aarch64-unknown-linux-gnu > /rust-target; echo aarch64-linux-gnu > /gcc-pkg; echo aarch64-linux-gnu-gcc > /gcc ;; \
      *) echo "unsupported architecture $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && apt-get update && apt-get install -y --no-install-recommends "gcc-$(cat /gcc-pkg)" "libc6-dev-${TARGETARCH}-cross" \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$(cat /rust-target)"
WORKDIR /src/server
COPY server/ ./
# The image's Rust is the build's Rust; the toolchain file is for development and CI.
RUN rm -f rust-toolchain.toml
# The registry cache is shared by both architectures, which build in parallel; `locked` stops them
# unpacking the same crate at once ("failed to unpack package ... File exists").
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/server/target,id=switchboard-target-${TARGETARCH} \
    target="$(cat /rust-target)" gcc="$(cat /gcc)" \
 && env_target="$(echo "$target" | tr a-z- A-Z_)" \
 && export "CARGO_TARGET_${env_target}_LINKER=$gcc" "CC_$(echo "$target" | tr - _)=$gcc" \
 && cargo build --release --locked --target "$target" \
 && cp "target/$target/release/switchboard" /switchboard \
 && mkdir /data

# ---- Runtime --------------------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=server /switchboard /usr/local/bin/switchboard
COPY --from=web /src/web/dist /app/web
# The database lives in /data; mount a volume there.
COPY --from=server --chown=65532:65532 /data /data
ENV SWITCHBOARD_LISTEN=0.0.0.0:8080 \
    SWITCHBOARD_DB=/data/switchboard.db \
    SWITCHBOARD_WEB_DIR=/app/web
EXPOSE 8080
VOLUME /data
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s CMD ["switchboard", "health"]
ENTRYPOINT ["switchboard"]
CMD ["serve"]

LABEL org.opencontainers.image.title="Switchboard" \
      org.opencontainers.image.description="Notification server: one endpoint in, routed to webhooks, Discord, displays and more" \
      org.opencontainers.image.licenses="MIT"
