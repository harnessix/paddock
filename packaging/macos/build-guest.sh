#!/bin/sh
# Build the guest agent for the macOS guest VM, sign it ad hoc, and write
# its SHA-256 next to it, for the `agent` share of a session. The agent
# in the guest checks this hash before it replaces itself with the new
# build.
#
# The signature names the designated requirement `identifier
# "dev.paddock.guest"`. The TCC row of the agent's Accessibility permission
# in the guest keeps the requirement of the binary that asked; an ad-hoc
# signature without this one would pin the hash of that one build, and
# every self-update of the agent would lose the permission.
#
# Usage: packaging/macos/build-guest.sh [OUT_DIR]
# OUT_DIR defaults to target/guest. It gets paddock-guest and
# paddock-guest.sha256.
set -eu
cd "$(dirname "$0")/../.."
out=${1:-target/guest}
cargo build --release --target aarch64-apple-darwin --bin paddock-guest
mkdir -p "$out"
cp "${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/release/paddock-guest" "$out/paddock-guest"
codesign --force --sign - --identifier dev.paddock.guest \
  -r='designated => identifier "dev.paddock.guest"' "$out/paddock-guest"
codesign -d -r- "$out/paddock-guest" 2>&1 | grep -q 'designated => identifier "dev.paddock.guest"' || {
  echo "build-guest: the signature has no identifier requirement" >&2
  exit 1
}
(cd "$out" && shasum -a 256 paddock-guest | awk '{print $1}' > paddock-guest.sha256)
echo "built $out/paddock-guest ($(cat "$out/paddock-guest.sha256"))"
