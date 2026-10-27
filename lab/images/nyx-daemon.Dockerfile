# nyx-daemon image for the Docker leak lab: headless Nyx (no Tauri, no UI) on Linux.
# Build from the repo root (the context must be the whole repo for the Cargo workspace):
#   docker build -f lab/images/nyx-daemon.Dockerfile -t nyx-lab/daemon .
# The first build is long: vcpkg compiles boost, openssl and libtorrent from source.
# That step only depends on vcpkg.json, so code changes reuse its cached layer.

# ---- build stage -------------------------------------------------------------------------------
# Same Rust as the Windows dev machine; trixie = current Debian stable, so the glibc the binary
# is linked against matches the debian:stable-slim runtime stage below.
FROM rust:1.99-trixie AS build

# vcpkg needs git/curl/zip to fetch sources and tools; the openssl port builds with perl + make.
RUN apt-get update && apt-get install -y --no-install-recommends \
        git curl zip unzip tar pkg-config make perl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# vcpkg pinned to the commit that is vcpkg.json's builtin-baseline (the same checkout the Windows
# toolchain uses). A full clone: manifest mode reads older port versions from the git history,
# which the libtorrent 2.0.11 override needs.
ARG VCPKG_COMMIT=a2f9862da6d88040a8537fb9b8cb24deaa9a196b
ENV VCPKG_ROOT=/opt/vcpkg
RUN git clone https://github.com/microsoft/vcpkg.git "$VCPKG_ROOT" \
    && git -C "$VCPKG_ROOT" checkout "$VCPKG_COMMIT" \
    && "$VCPKG_ROOT/bootstrap-vcpkg.sh" -disableMetrics

# Install libtorrent (+ boost, openssl) from this repo's manifest, triplet x64-linux (static libs).
# Installed into $VCPKG_ROOT/installed, where the vcpkg crate in nyx-engine's build.rs looks.
# Only vcpkg.json is copied, so this layer is rebuilt only when the manifest changes.
WORKDIR /manifest
COPY vcpkg.json .
RUN "$VCPKG_ROOT/vcpkg" install --triplet x64-linux --x-install-root="$VCPKG_ROOT/installed" \
    && rm -rf "$VCPKG_ROOT/buildtrees" "$VCPKG_ROOT/downloads" "$VCPKG_ROOT/packages"

# Build and test only the daemon. The workspace also lists src-tauri; Cargo reads its manifest
# to resolve the workspace but never builds it (or its Windows/WebView deps) with -p.
WORKDIR /src
COPY . .
RUN cargo build --release -p nyx-daemon
# The daemon's tests run as a build step: a failing test fails the image build.
RUN cargo test -p nyx-daemon
# Linux-only path rules the daemon relies on (the Windows dev machine never runs these).
RUN cargo test -p nyx-app --lib checked_save_dir

# ---- runtime stage -----------------------------------------------------------------------------
# Only the binary. libtorrent/boost/openssl are linked in statically; libstdc++ and glibc come
# from the base image (same Debian release as the build stage).
FROM debian:stable-slim
COPY --from=build /src/target/release/nyx-daemon /usr/local/bin/nyx-daemon
CMD ["nyx-daemon", "run"]
