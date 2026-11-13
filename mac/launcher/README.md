# paddock for Mac

You need macOS 14 or newer, Xcode 15.3 or newer, and XcodeGen (`brew install xcodegen`). Run each command from this folder.

Generate the Xcode project:

```sh
xcodegen generate
```

Run the PaddockKit tests:

```sh
(cd PaddockKit && swift test)
```

Build the app and run the unit and UI tests:

```sh
xcodebuild -project PaddockLauncher.xcodeproj -scheme PaddockLauncher -destination 'platform=macOS' test
```

Check the string catalog:

```sh
python3 Scripts/strings.py --check
```

Do all the checks above with warnings as errors:

```sh
Scripts/check-build.sh
```

Archive, sign and check the release build:

```sh
NOTARY_PROFILE=paddock Scripts/check-build.sh --release "Developer ID Application: NAME (TEAMID)"
```

Run the checks against the real paddock:

```sh
Scripts/nightly-real.sh APP [SHIM_BUNDLE_ID]
```

## The paddock check

paddock for Mac runs two commands on the paddock that it finds, in this order:

1. `paddock --version`. The version must be 0.2.2 or newer, with major version 0.
2. `paddock --launcher-protocol` on the same binary. The answer must be one JSON object with `"protocol":"paddock.mac-launcher/v1"` and every feature in `LauncherProtocol.required`.

Each command has a time limit of 15 seconds and an output limit of 16 KiB. Past a limit, or on Check Again, paddock for Mac kills the child process. The app stays unusable until both checks pass. The released paddock 0.2.2 does not know `--launcher-protocol`, so paddock for Mac asks you to update paddock.

## The files of paddock core

This folder builds and tests without the rest of paddock core. `Fixtures/core` holds copies of core fixtures. `core.pin` names the core version and commit that they come from.

To move the pin:

1. Copy `tests/fixtures/launcher/*.jsonl` of the new core commit into `Fixtures/core/launcher`, and `tests/fixtures/launcher/json/*.json` into `Fixtures/core/launcher/json`.
2. Copy `tests/fixtures/launcher-protocol/*.json` and `*.err` into `Fixtures/core/launcher-protocol`.
3. Change `version` and `commit` in `core.pin`.
4. Run the PaddockKit tests.
