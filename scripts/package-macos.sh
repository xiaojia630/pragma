#!/bin/sh
# Builds pragma for Apple Silicon and Intel, joins them into one binary, and packages it with the
# user docs and the demo as target/pragma-<version>-macos.zip.
set -e
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
targets="aarch64-apple-darwin x86_64-apple-darwin"

rustup target add $targets >/dev/null
for target in $targets; do
  cargo build --release -p pragma-cli --target "$target"
done

stage="target/package/pragma"
rm -rf target/package
mkdir -p "$stage"
lipo -create -output "$stage/pragma" \
  target/aarch64-apple-darwin/release/pragma target/x86_64-apple-darwin/release/pragma
codesign --force -s - "$stage/pragma"

# Same layout as docs/pragma/, so the README's links work, with the demo next to the binary.
cp docs/pragma/README.md docs/pragma/commands.md docs/pragma/how-it-works.md "$stage/"
cp docs/pragma/demo/try-demo.sh "$stage/try-demo.sh"
chmod +x "$stage/pragma" "$stage/try-demo.sh"

zip="target/pragma-$version-macos.zip"
rm -f "$zip"
ditto -c -k --keepParent "$stage" "$zip"
echo "$zip ($(lipo -archs "$stage/pragma"))"
