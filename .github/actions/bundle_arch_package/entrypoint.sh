#!/usr/bin/env bash

BUILD_ARCH="$2"
BUILD_ARCH="${BUILD_ARCH:-$(uname -m)}"

# Ensure we build with the most up-to-date package list. This could get stale due to Docker
# filesystem caching.
sudo pacman -Sy

# Run the bundle script with the release tag, skipping
# building the binary (as we have already done so), and only bundling the
# Arch package.
./script/bundle --release-tag "$1" --skip-build --packages arch --arch "$BUILD_ARCH"
