# Native, static Conductor built from the same verified source as desktop packages.
FROM golang:1.26.5-alpine@sha256:0178a641fbb4858c5f1b48e34bdaabe0350a330a1b1149aabd498d0699ff5fb2 AS conductor
RUN apk add --no-cache curl
WORKDIR /source
RUN curl --fail --silent --show-error --location https://codeload.github.com/hyperme-sh/hypermesh-conductor/tar.gz/8c381046952863e8ba83f054d467402c959eb59a -o source.tar.gz && \
    echo 'f19b825b28940b09b45c73170c7a13805b23e8b053f0b9ca2430137432a032de  source.tar.gz' | sha256sum -c - && \
    tar xzf source.tar.gz --strip-components=1 && \
    CGO_ENABLED=0 go build -buildvcs=false -mod=readonly -trimpath -o /conductor ./cmd/conductor
FROM ghcr.io/goreleaser/nfpm:v2.47.0@sha256:a662cb167d7b6d3a83920c83d76b12d02b8ac5dd2c13e5c62c15270b23f6df0c AS nfpm
# Native musl unit/build lane; no emulation or desktop processes.
FROM rust:1.90.0-alpine3.22@sha256:b4b54b176a74db7e5c68fdfe6029be39a02ccbcfe72b6e5a3e18e2c61b57ae26 AS build
ARG EXPECTED_ARCH
RUN test "$(uname -m)" = "$EXPECTED_ARCH" && rustc -vV | grep "host: ${EXPECTED_ARCH}-unknown-linux-musl"
RUN apk add --no-cache build-base python3
WORKDIR /src
COPY Cargo.toml Cargo.lock LICENSE ./
COPY src src
COPY session session
COPY companion companion
COPY third_party third_party
COPY tests tests
COPY ci ci
COPY packaging packaging
COPY docs/headless.md docs/headless.md
COPY --from=conductor /conductor /bundle/conductor
COPY --from=conductor /source/LICENSE /bundle/conductor-LICENSE
COPY --from=nfpm /usr/bin/nfpm /usr/local/bin/nfpm
RUN cargo tree --locked -p hypermesh-visor --no-default-features --features headless --prefix none > /tmp/dependencies && \
    ! grep -E '^(x11rb|zbus|keyring|secret-service) ' /tmp/dependencies
RUN cargo test --locked -p hypermesh-session --no-default-features --lib && \
    cargo test --locked -p hypermesh-visor --no-default-features --features headless --lib && \
    cargo build --release --locked -p hypermesh-visor --no-default-features --features headless
RUN readelf -l target/release/hypermesh-visor > /tmp/program-headers && ! grep -q INTERP /tmp/program-headers
ARG SOURCE_REVISION
RUN readelf -l /bundle/conductor > /tmp/conductor-headers && ! grep -q INTERP /tmp/conductor-headers
RUN EXPECTED_ARCH="$EXPECTED_ARCH" SOURCE_REVISION="$SOURCE_REVISION" sh packaging/headless/package.sh
FROM scratch AS artifacts
COPY --from=build /output/ /
