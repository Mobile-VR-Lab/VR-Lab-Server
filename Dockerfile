FROM ubuntu:22.04

# Make APT shut up.
ENV DEBIAN_FRONTEND=noninteractive
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# Set up the system.
RUN apt update -y && apt upgrade -y && apt install -y curl build-essential libssl-dev

# Install Rust
RUN curl --proto 'https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.75.0 -y \
    && mkdir -p /srv/vrlab

WORKDIR /srv/vrlab

# Add project files.
COPY Dockerfile entrypoint.sh Cargo.toml .
COPY src src
COPY test_client test_client

RUN /root/.cargo/bin/cargo install --path .

ENTRYPOINT ./entrypoint.sh