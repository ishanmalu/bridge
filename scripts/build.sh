#!/bin/sh
# Builds dist/Bridge.app (universal) and the release archives.
# The Windows exe comes from CI (MSVC, self-tested on a real Windows machine) for the commit being
# built: push first, wait for CI, then run this. BRIDGE_DEV=1 uses a local MinGW build instead,
# which needs WebView2Loader.dll next to it and must never be published.
set -e
cd "$(dirname "$0")/.."
cargo test --quiet
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

rm -rf dist && mkdir -p dist/Bridge.app/Contents/MacOS
# Universal: runs on Apple silicon and Intel Macs.
lipo -create -output dist/Bridge.app/Contents/MacOS/bridge \
  target/aarch64-apple-darwin/release/bridge target/x86_64-apple-darwin/release/bridge
mkdir -p dist/Bridge.app/Contents/Resources && cp assets/icon.icns dist/Bridge.app/Contents/Resources/Bridge.icns
cat > dist/Bridge.app/Contents/Info.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Bridge</string>
  <key>CFBundleIdentifier</key><string>dev.ishanmalu.bridge</string>
  <key>CFBundleExecutable</key><string>bridge</string>
  <key>CFBundleIconFile</key><string>Bridge</string>
  <key>CFBundleVersion</key><string>$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
# Sign with a stable certificate so macOS keeps the Accessibility grant across updates
# (an ad-hoc signature changes every build, and macOS would forget the permission).
# Set BRIDGE_SIGN_ID to a code-signing identity; falls back to ad-hoc.
SIGN_ID="${BRIDGE_SIGN_ID:-Perch Dev}"
if security find-identity -v -p codesigning | grep -q "\"$SIGN_ID\""; then
  codesign --force --deep --options runtime --sign "$SIGN_ID" dist/Bridge.app
else
  echo "warning: no '$SIGN_ID' identity; signing ad-hoc (Accessibility resets on update)"
  codesign --force --sign - dist/Bridge.app
fi
if [ -n "$BRIDGE_DEV" ]; then
  cargo build --release --target x86_64-pc-windows-gnu
  cp target/x86_64-pc-windows-gnu/release/bridge.exe dist/Bridge.exe
else
  SHA=$(git rev-parse HEAD)
  [ -z "$(git status --porcelain -- src assets Cargo.toml Cargo.lock build.rs)" ] || { echo "error: commit your changes first; the Windows exe is built by CI from a commit"; exit 1; }
  RUN=$(gh run list --commit "$SHA" --workflow CI --status success --json databaseId -q '.[0].databaseId')
  [ -n "$RUN" ] || { echo "error: no successful CI run for $SHA yet (push and wait for it)"; exit 1; }
  rm -rf target/ci-win && gh run download "$RUN" --name Bridge-windows --dir target/ci-win
  cp target/ci-win/bridge.exe dist/Bridge.exe
fi

# Release archives: ditto keeps the signature and bundle intact.
(cd dist && ditto -c -k --keepParent Bridge.app Bridge-mac.zip && zip -q Bridge-windows.zip Bridge.exe && cp Bridge.exe Bridge-windows.exe)
(cd dist && shasum -a 256 Bridge-mac.zip Bridge-windows.zip Bridge-windows.exe > SHA256SUMS.txt)

# Sign the checksum list; the app refuses updates that don't verify against the built-in key.
KEY="$HOME/.config/bridge-release/ed25519.pem"
OPENSSL=/opt/homebrew/opt/openssl@3/bin/openssl
if [ -f "$KEY" ]; then
  "$OPENSSL" pkeyutl -sign -rawin -inkey "$KEY" -in dist/SHA256SUMS.txt -out dist/SHA256SUMS.txt.sig
else
  echo "warning: no release key at $KEY; this build can't be published as an update"
fi
echo "Built dist/: Bridge.app, Bridge.exe, Bridge-mac.zip, Bridge-windows.zip, Bridge-windows.exe, SHA256SUMS.txt(.sig)"
