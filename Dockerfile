# Build the `farm` CLI as a fully-static musl binary.
#
# Portable across Docker and Podman:
#   - No BuildKit-only features (no `# syntax=` directive, no
#     `RUN --mount=type=cache`, no `--output type=local`). The image
#     builds identically under `docker build`, `docker buildx build`,
#     and `podman build`.
#   - Multi-arch: the host's `--platform` flag (and QEMU emulation for
#     cross-arch) drive the target. `TARGETARCH` is honoured when
#     supplied (buildx supplies it automatically; under plain docker
#     build / podman build, pass `--build-arg TARGETARCH=amd64|arm64`).
#
# Build:
#   docker build --platform linux/amd64 --build-arg TARGETARCH=amd64 -t farm-builder farm/
#   podman build --platform linux/amd64 --build-arg TARGETARCH=amd64 -t farm-builder farm/
#
# Extract the binary (works the same on both runtimes):
#   id=$(docker create farm-builder) && docker cp "$id:/farm" ./dist/farm && docker rm "$id"
#   id=$(podman create farm-builder) && podman cp "$id:/farm" ./dist/farm && podman rm "$id"
#
# The `Farmfile` in this directory wraps the common cases as
# `linux-amd64-{docker,podman}` / `linux-arm64-{docker,podman}` operations.

# ---- Build stage ---------------------------------------------------------
# Pin the Rust toolchain by minor version. Bump when you bump the
# `edition` / MSRV in Cargo.toml.
FROM rust:1.91.1-alpine AS builder

# TARGETARCH is supplied automatically by `docker buildx`. Under plain
# `docker build` or `podman build` pass `--build-arg TARGETARCH=amd64|arm64`.
# Defaults to amd64 when unset so a bare `docker build` still works.
ARG TARGETARCH=amd64

# musl-dev is the musl libc headers; pkgconfig + openssl-dev are
# needed transitively by reqwest's TLS path. git is needed because
# cargo resolves git deps at fetch time.
RUN apk add --no-cache \
    musl-dev \
    pkgconfig \
    openssl-dev \
    openssl-libs-static \
    git \
    bash

# Map Docker's TARGETARCH (amd64|arm64) to Rust target triples and
# install the matching musl target. Cross-arch builds work via QEMU.
RUN case "${TARGETARCH}" in \
        amd64) echo x86_64-unknown-linux-musl  > /tmp/rust-target ;; \
        arm64) echo aarch64-unknown-linux-musl > /tmp/rust-target ;; \
        *)     echo "Unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;; \
    esac && rustup target add "$(cat /tmp/rust-target)"

WORKDIR /src

# farm/ is fully self-contained.
COPY . .

# Build. The release-musl profile keeps LTO off so cross-compile
# emulation under QEMU doesn't OOM on small dev machines.
# No `--mount=type=cache` here — that's a BuildKit-only feature and
# would break podman + classic docker.
RUN TARGET="$(cat /tmp/rust-target)" && \
    cargo build --release --target "$TARGET" --bin farm && \
    cp "target/${TARGET}/release/farm" /farm && \
    strip /farm 2>/dev/null || true

# ---- Final stage ---------------------------------------------------------
# Plain `scratch` image carrying only the static binary at `/farm`.
# Extract it with `{docker,podman} create <image>` + `cp`. (We avoid
# `--output type=local` because it's BuildKit-only.)
FROM scratch
COPY --from=builder /farm /farm
