FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
    libssl-dev pkg-config xvfb clang lld llvm nodejs npm openssh-server \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt && cargo install cargo-xwin --locked
