#!/usr/bin/env bash
# System libraries gpui needs to build on Debian/Ubuntu (X11 + Wayland,
# fontconfig/freetype text, Vulkan via wgpu).
set -euo pipefail

SUDO=""
if [[ $(id -u) -ne 0 ]]; then SUDO="sudo"; fi

$SUDO apt-get update
$SUDO apt-get install -y --no-install-recommends \
    build-essential \
    clang \
    cmake \
    pkg-config \
    libfontconfig-dev \
    libfreetype-dev \
    libssl-dev \
    libvulkan-dev \
    libwayland-dev \
    libx11-xcb-dev \
    libxcb1-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-dev
