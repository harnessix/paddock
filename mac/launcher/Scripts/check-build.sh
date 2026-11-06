#!/bin/bash
# Build check of paddock for Mac, on a Mac with Xcode 15.3 or newer and
# XcodeGen (brew install xcodegen).
#
#   check-build.sh                 build with warnings as errors, run all tests
#   check-build.sh --release ID    also archive with the Developer ID identity
#                                  ID, verify the signature and Gatekeeper
#   NOTARY_PROFILE=name check-build.sh --release ID
#                                  also notarize a DMG, staple it, validate it
#
# The release checks run as XCTest cases (ReleaseChecks) against the archive.
set -euo pipefail
cd "$(dirname "$0")/.."

identity=""
if [ "${1:-}" = "--release" ]; then
  identity=${2:?give the Developer ID Application identity}
fi

python3 Scripts/strings.py --check
xcodegen generate --spec project.yml

out=Build
mkdir -p "$out"

echo "== PaddockKit tests"
(cd PaddockKit && swift test)

echo "== build and test with warnings as errors"
xcodebuild -project PaddockLauncher.xcodeproj -scheme PaddockLauncher \
  -destination 'platform=macOS' -derivedDataPath "$out/DerivedData" \
  SWIFT_TREAT_WARNINGS_AS_ERRORS=YES GCC_TREAT_WARNINGS_AS_ERRORS=YES \
  test

[ -n "$identity" ] || { echo "check-build: ok (no release identity given)"; exit 0; }

echo "== archive"
xcodebuild -project PaddockLauncher.xcodeproj -scheme PaddockLauncher \
  -derivedDataPath "$out/DerivedData" -archivePath "$out/PaddockLauncher.xcarchive" \
  SWIFT_TREAT_WARNINGS_AS_ERRORS=YES CODE_SIGN_IDENTITY="$identity" \
  archive
app="$out/PaddockLauncher.xcarchive/Products/Applications/paddock for Mac.app"

dmg=""
if [ -n "${NOTARY_PROFILE:-}" ]; then
  echo "== notarize"
  dmg="$out/paddock-for-Mac.dmg"
  rm -f "$dmg"
  hdiutil create -volname "paddock for Mac" -srcfolder "$app" -ov -format UDZO "$dmg"
  codesign --sign "$identity" --timestamp "$dmg"
  xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler staple "$app"
fi

echo "== release checks"
# xcodebuild passes TEST_RUNNER_ variables to the test process without the prefix.
TEST_RUNNER_PADDOCK_LAUNCHER_RELEASE_CHECKS=1 \
TEST_RUNNER_PADDOCK_LAUNCHER_ARCHIVED_APP="$PWD/$app" \
TEST_RUNNER_PADDOCK_LAUNCHER_DMG="${dmg:+$PWD/$dmg}" \
  xcodebuild -project PaddockLauncher.xcodeproj -scheme PaddockLauncher \
  -destination 'platform=macOS' -derivedDataPath "$out/DerivedData" \
  -only-testing:PaddockLauncherTests/ReleaseChecks test
echo "check-build: ok"
