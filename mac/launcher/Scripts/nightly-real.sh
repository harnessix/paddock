#!/bin/bash
# The checks against the real paddock on a Mac. They need a
# built base image and one app profile.
#
#   nightly-real.sh APP [SHIM_BUNDLE_ID]
#
# APP is the app profile to start (with --network none for the share check).
# SHIM_BUNDLE_ID is the bundle id of paddock's question panel, for the key
# check; without it that check is skipped.
set -euo pipefail
cd "$(dirname "$0")/.."
app=${1:?give an app profile name}
xcodegen generate --spec project.yml
# xcodebuild passes TEST_RUNNER_ variables to the test process without the prefix.
TEST_RUNNER_PADDOCK_LAUNCHER_REAL=1 \
TEST_RUNNER_PADDOCK_LAUNCHER_REAL_APP="$app" \
TEST_RUNNER_PADDOCK_LAUNCHER_REAL_SHIM="${2:-}" \
TEST_RUNNER_PADDOCK_LAUNCHER_REAL_BIN="$(command -v paddock)" \
  xcodebuild -project PaddockLauncher.xcodeproj -scheme PaddockLauncher \
  -destination 'platform=macOS' -derivedDataPath Build/DerivedData \
  -only-testing:PaddockLauncherUITests/NightlyRealTests test
