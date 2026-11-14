# syntax=docker/dockerfile:1
ARG RUNTIME_BASE=debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
FROM golang:1.26.5-alpine@sha256:0178a641fbb4858c5f1b48e34bdaabe0350a330a1b1149aabd498d0699ff5fb2 AS cli
WORKDIR /cli
COPY third_party/hypermesh-cli ./
RUN CGO_ENABLED=0 go test -mod=readonly ./... && \
    CGO_ENABLED=0 go build -mod=readonly -buildvcs=false -trimpath -o /out/hypermesh ./cmd/hypermesh && \
    /out/hypermesh --help
FROM rust:1.90.0-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f AS toolchain
FROM toolchain AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY session session
COPY local-storage local-storage
COPY local-transport local-transport
COPY native/crash/guard native/crash/guard
COPY inverse-client inverse-client
COPY companion companion
COPY shared shared
COPY third_party third_party
COPY tests tests
RUN cargo build --locked --release --workspace

FROM build AS crash-sdk
RUN apt-get update && apt-get install -y --no-install-recommends cmake libcurl4-openssl-dev python3 && rm -rf /var/lib/apt/lists/*
COPY native/crash native/crash
COPY LICENSE LICENSE
RUN bash native/crash/build-sdk.sh /crash-sdk && \
    python3 native/crash/stage-runtime.py /crash-sdk /crash-runtime --platform linux --revision "$(cat /crash-sdk/visor-source-revision)"

FROM ${RUNTIME_BASE} AS desktop
COPY --from=toolchain /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ARG DEBIAN_SUITE=trixie
ARG DEBIAN_SNAPSHOT=20261102T000000Z
# Snapshot archives pin dependencies as well as the base. Signature checks stay on.
RUN rm -f /etc/apt/sources.list.d/debian.sources && \
    printf 'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/%s/ %s main\ndeb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/%s/ %s-security main\n' \
      "$DEBIAN_SNAPSHOT" "$DEBIAN_SUITE" "$DEBIAN_SNAPSHOT" "$DEBIAN_SUITE" > /etc/apt/sources.list && \
    apt-get -o Acquire::ForceIPv4=true -o Acquire::https::Timeout=30 -o APT::Update::Error-Mode=any -o Acquire::https::CaInfo=/etc/ssl/certs/ca-certificates.crt update && apt-get -o Acquire::ForceIPv4=true -o Acquire::https::Timeout=30 -o APT::Update::Error-Mode=any -o Acquire::https::CaInfo=/etc/ssl/certs/ca-certificates.crt install -y --no-install-recommends \
      ca-certificates libcurl4t64 dbus-x11 firefox-esr fonts-dejavu-core mousepad openbox \
      python3 python3-tk python3-xlib pulseaudio tini x11-utils xauth xdotool xterm xvfb zenity && \
    dpkg-query -W > /usr/share/visor-packages.txt && rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 2000 resources && useradd --uid 1000 --create-home --shell /bin/bash bot && \
    usermod -a -G resources bot && install -d -o bot -g bot -m 0700 /run/visor /home/bot/.config /home/bot/.local/state /home/bot/workspace
COPY --from=build /src/target/release/hypermesh-visor /src/target/release/hypermesh-companion /src/target/release/hypermesh-desktop-mcp /src/target/release/hypermesh-acp /usr/local/bin/
COPY --from=cli /out/hypermesh /usr/local/bin/hypermesh
COPY --from=crash-sdk /crash-runtime /usr/local/lib/hypermesh-visor/crash-sdk
RUN ldd /usr/local/lib/hypermesh-visor/crash-sdk/bin/crashpad_handler > /tmp/crash-dependencies && \
    ! grep -q 'not found' /tmp/crash-dependencies && rm /tmp/crash-dependencies
COPY third_party/hypermesh-cli/VENDOR_REV /usr/share/doc/hypermesh-cli/VENDOR_REV
RUN install -d -o bot -g resources -m 2770 /resources/share
COPY container/entrypoint.py /usr/local/bin/visor-container
COPY container/unattended.py /usr/local/bin/visor_unattended.py
COPY container/resource.py /usr/local/bin/visor-resource
COPY container/health.py /usr/local/bin/visor-health
RUN chmod 0755 /usr/local/bin/visor-container /usr/local/bin/visor-health /usr/local/bin/visor-resource
ENV HOME=/home/bot DISPLAY=:99 XAUTHORITY=/run/visor/Xauthority XDG_RUNTIME_DIR=/run/visor \
    XDG_CONFIG_HOME=/home/bot/.config XDG_STATE_HOME=/home/bot/.local/state \
    HYPERMESH_RUNTIME_DIR=/run/visor/hypermesh HYPERMESH_CONFIG_DIR=/home/bot/.config/hypermesh \
    PULSE_SERVER=unix:/run/visor/pulse/native
USER 1000:1000
WORKDIR /home/bot/workspace
HEALTHCHECK --interval=5s --timeout=3s --start-period=30s --retries=3 CMD ["visor-health"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/bin/dbus-run-session", "--", "/usr/local/bin/visor-container"]
