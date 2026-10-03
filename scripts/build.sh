#!/bin/sh
# Builds dist/Bridge.app (Apple silicon) and dist/bridge.exe (Windows x64).
# Needs: rustup target add x86_64-pc-windows-gnu && brew install mingw-w64
set -e
cd "$(dirname "$0")/.."
cargo test --quiet
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target x86_64-pc-windows-gnu

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
# Ad-hoc signature: macOS ties the Accessibility grant to it, so re-grant after each rebuild.
codesign --force --sign - dist/Bridge.app
cp target/x86_64-pc-windows-gnu/release/bridge.exe dist/Bridge.exe

# Release archives: ditto keeps the signature and bundle intact.
(cd dist && ditto -c -k --keepParent Bridge.app Bridge-mac.zip && zip -q Bridge-windows.zip Bridge.exe)
(cd dist && shasum -a 256 Bridge-mac.zip Bridge-windows.zip > SHA256SUMS.txt)
echo "Built dist/Bridge.app, dist/Bridge.exe, dist/Bridge-mac.zip, dist/Bridge-windows.zip"
