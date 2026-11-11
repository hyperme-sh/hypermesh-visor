# Native musl unit/build lane; no emulation or desktop processes.
FROM rust:1.90.0-alpine3.22@sha256:b4b54b176a74db7e5c68fdfe6029be39a02ccbcfe72b6e5a3e18e2c61b57ae26 AS build
ARG EXPECTED_ARCH
RUN test "$(uname -m)" = "$EXPECTED_ARCH" && rustc -vV | grep "host: ${EXPECTED_ARCH}-unknown-linux-musl"
RUN apk add --no-cache build-base python3
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY session session
COPY companion companion
COPY third_party third_party
COPY tests tests
COPY ci ci
RUN cargo tree --locked -p hypermesh-visor --no-default-features --features headless --prefix none > /tmp/dependencies && \
    ! grep -E '^(x11rb|zbus|keyring|secret-service) ' /tmp/dependencies
RUN cargo test --locked -p hypermesh-session --no-default-features --lib && \
    cargo test --locked -p hypermesh-visor --no-default-features --features headless --lib && \
    cargo build --release --locked -p hypermesh-visor --no-default-features --features headless
RUN readelf -l target/release/hypermesh-visor > /tmp/program-headers && ! grep -q INTERP /tmp/program-headers
RUN mkdir /output && cp target/release/hypermesh-visor /output/hypermesh-visor-headless && \
    cd /output && sha256sum hypermesh-visor-headless > checksums.txt
FROM scratch AS artifacts
COPY --from=build /output/ /
