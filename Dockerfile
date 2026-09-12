# syntax=docker/dockerfile:1
# Static musl build: zero runtime dependencies (thanks to rustls instead
# of OpenSSL). The final binary runs inside a "scratch" image of just a
# few dozen MBs in total.
#
# Note: `--platform=$BUILDPLATFORM` is omitted here on purpose. When running
# `docker buildx build --platform linux/amd64,linux/arm64`, each builder stage
# executes natively (or via QEMU) on the target platform. This ensures the musl
# rust target always matches the builder host architecture — no manual
# cross-linker configuration required.
FROM rust:1-alpine AS builder
# ca-certificates is needed here so `cargo build` can fetch crates over
# HTTPS from crates.io during the build — it is NOT copied into the final
# image (see note below).
RUN apk add --no-cache musl-dev ca-certificates
WORKDIR /app

# ui-dist/ must exist BEFORE the build: rust-embed embeds it into the
# binary at compile time (only during a `--release` build).
COPY . .
RUN RUST_TARGET="$(rustc -vV | sed -n 's/host: //p')" && \
    cargo build --release --target "$RUST_TARGET" && \
    cp "target/$RUST_TARGET/release/sbalite" /sbalite

FROM scratch
# NOTE: sbalite is built with reqwest's "rustls-tls-webpki-roots" feature,
# which bundles Mozilla's CA list directly into the binary at compile
# time. It never reads a system certificate store at runtime, so there is
# nothing to copy here — /etc/ssl/certs would simply be dead weight.
# To trust an additional/internal CA at runtime, use the `ca_cert_path`
# setting in instances.toml instead (see README).
COPY --from=builder /sbalite /sbalite

# Mount your instances.toml file to this path (or override via INSTANCES_CONFIG).
ENV INSTANCES_CONFIG=/config/instances.toml

EXPOSE 9001

ENTRYPOINT ["/sbalite"]