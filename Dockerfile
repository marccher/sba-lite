# syntax=docker/dockerfile:1

# Build statica musl: nessuna dipendenza runtime (grazie a rustls al posto
# di OpenSSL), il binario finale gira su un'immagine "scratch" di poche
# decine di MB in totale — coerente con l'obiettivo di footprint minimo
# rispetto al backend Java originale (~8MB di RAM vs ~200MB+).
#
# Nota: nessun --platform=$BUILDPLATFORM qui di proposito. Con
# `docker buildx build --platform linux/amd64,linux/arm64` ogni stage builder
# gira nativamente (o via QEMU) sulla piattaforma target, così il target musl
# rust corrisponde sempre all'arch dell'host del builder — niente cross
# linker da configurare a mano.
FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
# ui-dist/ deve esistere PRIMA della build: rust-embed la incorpora nel
# binario a tempo di compilazione (solo in build --release).
COPY . .

RUN RUST_TARGET="$(rustc -vV | sed -n 's/host: //p')" && \
    cargo build --release --target "$RUST_TARGET" && \
    cp "target/$RUST_TARGET/release/sbalite" /sbalite

FROM scratch

COPY --from=builder /sbalite /sbalite

# Monta il tuo instances.toml in questo path (o sovrascrivi INSTANCES_CONFIG).
ENV INSTANCES_CONFIG=/config/instances.toml
EXPOSE 9000

ENTRYPOINT ["/sbalite"]
