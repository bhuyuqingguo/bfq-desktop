#!/usr/bin/env bash
set -euo pipefail

test "$(dpkg --print-architecture)" = arm64
target=aarch64-unknown-linux-musl
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo build --locked --release --target "$target" --bin guanhai-core
core="target/$target/release/guanhai-core"
readelf --file-header "$core" | grep 'AArch64'
readelf --program-headers --wide "$core" > /tmp/guanhai-elf-headers
! grep -q INTERP /tmp/guanhai-elf-headers || exit 1
readelf --dynamic --wide "$core" > /tmp/guanhai-elf-dynamic
! grep -q NEEDED /tmp/guanhai-elf-dynamic || exit 1

mkdir -p desktop/release
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
install -d "$stage/DEBIAN"
install -Dm755 desktop/kylin/guanhai-bafen-v10 "$stage/usr/bin/guanhai-bafen"
install -Dm755 "$core" "$stage/usr/lib/guanhai-bafen/guanhai-core"
install -Dm644 desktop/kylin/com.guanhai.bafen.desktop \
  "$stage/usr/share/applications/com.guanhai.bafen.desktop"
install -Dm644 desktop/src-tauri/icons/128x128.png \
  "$stage/usr/share/icons/hicolor/128x128/apps/com.guanhai.bafen.png"
cat > "$stage/DEBIAN/control" <<'CONTROL'
Package: guanhai-bafen
Version: 0.0.1+kylin0
Architecture: arm64
Maintainer: Guanhai Bafen
Section: utils
Priority: optional
Depends: python3 (>= 3.5), python3-gi, gir1.2-gtk-3.0
Description: Guanhai Bafen local file encryption and legacy archive recovery
 Native GTK desktop edition for the original Kylin V10 ARM64.
CONTROL
desktop-file-validate "$stage/usr/share/applications/com.guanhai.bafen.desktop"
dpkg-deb -Zxz --root-owner-group --build "$stage" desktop/release/guanhai-bafen-kylin-v10-arm64.deb
(cd desktop/release && sha256sum guanhai-bafen-kylin-v10-arm64.deb > guanhai-bafen-kylin-v10-arm64.sha256)
