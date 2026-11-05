# syntax=docker/dockerfile:1
# Bothy hub image: the program only. Content downloads into the /data volume on first start
# (bothy-hub get), so program updates are small pulls and the library stays put.
#
#   docker build -f hub/Dockerfile -t bothy-hub .        (from the repo root)
#   docker run -d --name bothy --restart unless-stopped -p 8080:8080 -p 8443:8443 -v bothy-data:/data bothy-hub

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang libclang-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY core/ core/
COPY app/assets/emergency/cards.json app/assets/emergency/cards.json
COPY app/assets/reader/ app/assets/reader/
ARG TARGETARCH
# Portable CPU baselines. core/.cargo/config.toml builds for the *host* CPU (target-cpu=native) on arm64,
# which put SVE in the image and crashed it on Apple silicon (exit 132): RUSTFLAGS overrides it.
# arm64 -> armv8-a (llama.cpp's default without target-cpu; runs on a Pi 4 and every Mac), amd64 -> x86-64-v2.
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/core/target,id=bothy-target-$TARGETARCH \
    cd core && if [ "$TARGETARCH" = "amd64" ]; then export RUSTFLAGS="-C target-cpu=x86-64-v2"; else export RUSTFLAGS=""; fi \
 && CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=true cargo build --release -p sp-server --bin bothy-hub \
 && cp target/release/bothy-hub /bothy-hub

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libgomp1 && rm -rf /var/lib/apt/lists/*
COPY --from=build /bothy-hub /usr/local/bin/bothy-hub
ENV BOTHY_DATA=/data
VOLUME /data
EXPOSE 8080 8443
LABEL org.opencontainers.image.title="Bothy hub" \
      org.opencontainers.image.description="Offline knowledge, first aid, maps, games and chat for everyone on your Wi-Fi" \
      org.opencontainers.image.url="https://www.pocketbothy.com/hub" \
      org.opencontainers.image.licenses="AGPL-3.0-only"
ENTRYPOINT ["bothy-hub"]
CMD ["start"]
