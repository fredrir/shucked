#!/bin/sh
# Target-native Linux build; the container runtime must supply any CPU emulation.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
target=${1:?Usage: build-container.sh linux-arm64|linux-armhf|linux-x64|alpine-arm64|alpine-x64}
case "$target" in
  linux-*) image='docker.io/library/rust@sha256:6e957ef098dcc77d33e310261e4ed5843bb108d5c3b5dc2b476cbc8b6caf53fa'; install='apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config python3 cmake curl git xz-utils patch perl bzip2' ;;
  alpine-*) image='public.ecr.aws/docker/library/alpine@sha256:e7a1a92a5bfeee40966aea60f0796b0e7917cc35591542701834f03a68fa3d18'; install='apk add build-base cargo rust cmake python3 curl git linux-headers xz bzip2 perl patch' ;;
  *) echo 'Unsupported Linux family' >&2; exit 1 ;;
esac
case "$target" in
  *-arm64) architecture=arm64 ;;
  linux-armhf) architecture=arm/v7; image='docker.io/library/rust@sha256:ff4de18a076127e0c6bbee2447278be467ebeaf007d9c759affc41c2b7e7a09f' ;;
  linux-x64) architecture=amd64; image='docker.io/library/rust@sha256:408fe88047cef61a2087653b0c5255fa51c0f2d6d94ddedd7a2562a9b91a46f6' ;;
  alpine-x64) architecture=amd64; image='public.ecr.aws/docker/library/alpine@sha256:d56c381f961d307a21b3ca004cf1e3910f106644aefb1f43e654c8a56c4fd395' ;;
  *) echo 'Unsupported architecture' >&2; exit 1 ;;
esac
image=${SHUCKED_BUILD_IMAGE:-$image}
case "$image" in *@sha256:*) ;; *) echo 'Build image must be digest-pinned' >&2; exit 1;; esac
engine=${SHUCKED_CONTAINER_ENGINE:-podman}
output=${SHUCKED_PROVIDER_DEST:-$repo/target/provider-$target}
build=${SHUCKED_PROVIDER_BUILD:-$repo/target/provider-container-build-$target}
mkdir -p "$output" "$build"
"$engine" run --rm --platform="linux/$architecture" -v "$repo:/repo" -v "$output:/output" -v "$build:/build" \
  -e SHUCKED_PROVIDER_DEST=/output -e SHUCKED_PROVIDER_BUILD=/build \
  -e SHUCKED_FISH_PREBUILT="${SHUCKED_FISH_PREBUILT:-}" \
  -e SHUCKED_PROVIDER_EXECUTION="${SHUCKED_PROVIDER_EXECUTION:-container}" \
  -e SHUCKED_BUILD_JOBS="${SHUCKED_BUILD_JOBS:-1}" -e FORCE_UNSAFE_CONFIGURE=1 \
  "$image" sh -c "$install && /repo/tooling/providers/build-unix.sh && python3 /repo/tooling/providers/runtime-manifest.py /output --target '$target'"
