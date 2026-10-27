#!/usr/bin/env bash
# Builds static Linux binaries locally and publishes them as a GitHub release.
# Version and tag come from Cargo.toml, so bump it there first.
#
# Requirements: zig, cargo-zigbuild, gh (logged in), rustup targets
#   x86_64-unknown-linux-musl and aarch64-unknown-linux-musl.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"
TARGETS=(x86_64 aarch64)

if gh release view "$TAG" >/dev/null 2>&1; then
    echo "Release $TAG already exists, bump the version in Cargo.toml first." >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
    echo "Working tree is dirty, commit your changes first." >&2
    exit 1
fi

cargo test --locked

rm -rf dist && mkdir dist
for arch in "${TARGETS[@]}"; do
    target="$arch-unknown-linux-musl"
    echo "==> Building $target"
    cargo zigbuild --release --locked --target "$target"
    cp "target/$target/release/cs2-tracker" "dist/cs2-tracker-linux-$arch"
done
(cd dist && sha256sum cs2-tracker-linux-* > SHA256SUMS)

git tag -a "$TAG" -m "$TAG"
git push origin HEAD "$TAG"
gh release create "$TAG" dist/* --title "$TAG" --generate-notes
echo "==> Published $TAG"
