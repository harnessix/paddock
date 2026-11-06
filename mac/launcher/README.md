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
