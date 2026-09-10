# Use Rust base image
FROM mcr.microsoft.com/devcontainers/rust:1

# Install additional dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    libclang-dev \
    libdbus-1-dev \
    gnome-keyring \
    libxcb1-dev \
    protobuf-compiler \
    && apt-get clean
