# Orbit Notes

A small SwiftUI iPhone/iPad app for trying `ios-release` before connecting an
Apple account. It includes a share extension, two unit tests, a shared scheme
with an `AppStore` archive configuration, English/Arabic string catalogs, and
app-owned UI screenshot tests.

The checked-in Xcode project is ready to open. XcodeGen is needed only when
editing `project.yml`; it is not part of app or CLI onboarding.

## Try unsigned QA

[Install the native CLI](../../docs/install.md). From a clone of this repository,
copy this example into a temporary folder so generated configuration and build
output stay outside the platform checkout:

```sh
task_example="$(mktemp -d)"
cp -R examples/OrbitNotes "$task_example/OrbitNotes"
cd "$task_example/OrbitNotes"
ios-release init --scheme OrbitNotes --no-workflows
ios-release qa all
```

This needs macOS, Xcode, and an installed iOS simulator runtime. It does not need
an Apple account, signing material, or a GitHub repository. `init` discovers your
installed toolchain and device. Review `.ios-release.json`; QA writes results to
`qa-results/` and build diagnostics to `build/`.

## Try screenshot capture

In the generated `.ios-release.json`, set `screens` to `["01-notes"]` and `locales`
to `["en-US", "ar-SA"]`. Set `screenshot_devices` to the installed device you want
to capture, then pass that exact name below:

```sh
ios-release screenshots --scheme OrbitNotes --test-target OrbitNotesUITests --only-testing OrbitNotesUITests/OnboardingTests/testStoreScreenshot --devices 'iPhone 18 Pro Max' --languages en-US,ar-SA --isolated-cache
```

Output is `build/rust-screenshots/images`. Repeat with an installed iPad after
adding it to `screenshot_devices`. The
[native helper](UITests/IosReleaseSnapshot.swift) uses an app-local cache and
checks the localized title, including Arabic right-to-left output.
[The screenshot guide](../../docs/native-screenshots.md) explains evidence and
hosted simulator recovery.

## Signing your own app

The fixture declares no tracking or non-exempt encryption. Those declarations
apply to this fixture; answer them for your own app. Choose unique bundle IDs and
your Apple team before any signed release. Follow the
[quickstart](../../docs/native-quickstart.md) for Apple access and Actions.

The platform's [adoption workflow](../../.github/workflows/native-adoption.yml)
rehearses QA and screenshots on fresh hosted runners. It does not distribute this
fixture to Apple or use PicStrip's credentials, project, or active simulators.
