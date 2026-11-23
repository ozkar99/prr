#!/bin/sh
# Builds "Review Tool.app" and a DMG in dist/.
#
#   scripts/bundle-macos.sh             this Mac's architecture
#   scripts/bundle-macos.sh universal   Apple Silicon + Intel (what releases ship)
#
# The app is one Rust binary plus the two language servers that run on Node.js
# (pyright for Python, bash-language-server for shell), installed into
# Contents/Resources/language-servers. Ad-hoc signed; see the README for
# signing and notarizing.
set -eu
cd "$(dirname "$0")/.."

PYRIGHT=1.1.414
BASH_LS=5.8.1
NAME="Review Tool"
ID=com.ozkar99.review-tool
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ "${1:-}" = universal ]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/purr \
    target/aarch64-apple-darwin/release/purr \
    target/x86_64-apple-darwin/release/purr
  BIN=target/universal/purr
  ARCH=universal
else
  cargo build --release
  BIN=target/release/purr
  ARCH=$(uname -m | sed 's/arm64/aarch64/')
fi

APP="dist/$NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/purr"
cp assets/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleExecutable</key><string>purr</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

# Language servers that run on Node.js (the app finds them in Resources).
npm install --prefix "$APP/Contents/Resources/language-servers" --omit=dev --no-audit --no-fund --no-package-lock \
  "pyright@$PYRIGHT" "bash-language-server@$BASH_LS" >/dev/null
rm -f "$APP/Contents/Resources/language-servers/package.json"
echo "bundled pyright $PYRIGHT, bash-language-server $BASH_LS"

codesign --force --deep --sign - "$APP"

# The app's binary is also the `purr` command; a symlink keeps it inside the
# bundle (it finds its language servers and the app through its real location).
rm -f dist/review-tool
ln -sf "$NAME.app/Contents/MacOS/purr" dist/purr

# The DMG name matches what GitHub serves for "Review Tool_…" uploads (the cask's URL).
DMG="dist/Review.Tool_${VERSION}_${ARCH}.dmg"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo "built $APP, $DMG and dist/purr (the command)"
