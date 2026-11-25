#!/usr/bin/env bash
set -euo pipefail

test "$(dpkg --print-architecture)" = arm64
cargo build --locked --release --bin guanhai-core
readelf --file-header target/release/guanhai-core | grep 'AArch64'
max_glibc=$(readelf --version-info target/release/guanhai-core |
  sed -nE 's/.*Name: GLIBC_([0-9.]+).*/\1/p' | sort -Vu | tail -1)
test -n "$max_glibc"
dpkg --compare-versions "$max_glibc" le 2.31

mkdir -p desktop/release
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
install -d "$stage/DEBIAN" "$stage/usr/lib/guanhai-bafen/ui"
cp -a desktop/dist/. "$stage/usr/lib/guanhai-bafen/ui/"
install -Dm755 desktop/kylin/guanhai-bafen "$stage/usr/bin/guanhai-bafen"
install -Dm755 target/release/guanhai-core "$stage/usr/lib/guanhai-bafen/guanhai-core"
install -Dm644 desktop/kylin/com.guanhai.bafen.desktop \
  "$stage/usr/share/applications/com.guanhai.bafen.desktop"
install -Dm644 desktop/src-tauri/icons/128x128.png \
  "$stage/usr/share/icons/hicolor/128x128/apps/com.guanhai.bafen.png"
# The installed core depends on this distribution's glibc and GCC runtime.
cat > "$stage/DEBIAN/control" <<'CONTROL'
Package: guanhai-bafen
Version: 0.0.1+kylin1
Architecture: arm64
Maintainer: Guanhai Bafen
Section: utils
Priority: optional
Depends: libc6 (>= 2.31), libgcc-s1, python3, python3-gi, gir1.2-gtk-3.0, gir1.2-webkit2-4.0
Description: Guanhai Bafen local file encryption and legacy archive recovery
 Desktop edition for Kylin V10 ARM64 with GTK 3 and WebKitGTK 4.0.
CONTROL
desktop-file-validate "$stage/usr/share/applications/com.guanhai.bafen.desktop"
dpkg-deb --root-owner-group --build "$stage" desktop/release/guanhai-bafen-kylin-v10-arm64.deb
