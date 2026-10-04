# Native Rust CLI

The Rust implementation is a standalone `ios-release` executable for native iOS and iPadOS releases. New apps use `.ios-release.json`; the existing `.github/ios-release.json` contract remains supported. Once compiled, it needs no Ruby, Python, Node, Bundler or uv runtime. Native builds require macOS and the configured Xcode; signing uses OpenSSL and Keychain tools, and lint requires SwiftLint. [Native quickstart](native-quickstart.md) covers onboarding through production management.

The native path is opt-in. Existing callers, bootstrap actions, Python entrypoints and Fastlane adapters retain their behavior. New native workflows separately support signing, authenticated preparation and Apple operations. PicStrip's default release path stays on the established adapter until fresh signed and live delivery parity are verified.

## Build and invoke explicitly

Install Rust using your package manager or [rustup](https://rustup.rs), then compile from a reviewed platform checkout:

```sh
cd rust
cargo build --locked --release
./target/release/ios-release doctor
./target/release/ios-release --commands-json
```

Use the binary by its full path while evaluating it. Do not replace the existing `ios-release` on PATH or update a consumer's workflow pin to activate this preview.

```sh
/path/to/platform/rust/target/release/ios-release --app-root /path/to/app qa test
/path/to/platform/rust/target/release/ios-release --app-root /path/to/app qa test-compatibility
/path/to/platform/rust/target/release/ios-release --app-root /path/to/app --plan archive --version 1.7.0 --build-number 77.1
```

`--config` accepts an app-relative or absolute configuration path. `IOS_RELEASE_CONFIG` is also supported. `--plan` prints intended commands without launching tools or changing files. Arguments pass directly to subprocesses; the CLI does not evaluate shell commands.

## Implemented commands

| Command | Behavior |
| --- | --- |
| `doctor` | Reports native version and runtime requirements without an app checkout |
| `doctor --xcode`, `toolchain` | Validates exact Xcode version/build and SDK; resolves unique available configured simulator UUIDs |
| `toolchain --compatibility` | Selects the configured compatibility Xcode and runtime |
| `toolchain --screenshots --destinations-json` | Resolves the configured screenshot devices |
| `qa lint`, `qa localization`, `qa analyze` | Runs SwiftLint, the native string catalog audit, or unsigned static analysis |
| `qa test`, `test`, `qa test-compatibility` | Runs explicitly configured targets with bounded readiness and validated XCTest evidence |
| `archive --version … --build-number …` | Archives and exports using installed manual signing assets; validates exported bundles, privacy policies, profiles, entitlements, certificates, and matching dSYMs |
| `screenshots`, `screenshots-capture` | Builds an explicit UI test scheme once per selected device and captures configured locales without rebuilding |
| `xcresult-report --summary … --tests …` | Reads exported XCTest JSON and prints reconciled results, JUnit, and bootstrap classification |
| `init`, `auth login` | Discovers an app and stores its Apple API key privately |
| `signing sync`, `signing import`, `signing status` | Reconciles owned certificates/profiles, imports existing identities and reports renewal state |
| `qa all`, `release prepare` | Runs configured QA and prepares a source-bound signed release |
| `github setup`, `github fetch` | Configures protected native environments or downloads an authenticated preparation and recovery receipt |
| `store upload`, `store wait`, `store testflight` | Transfers the exact IPA, reconciles Apple processing and assigns beta groups |
| `store stage`, `store metadata`, `store submit --confirm` | Selects the exact build, applies content and requests review |
| `store publish --confirm`, `store phased …`, `status` | Publishes an approved version, manages phased updates and reads production state |

QA keeps the existing `qa-results/<check>/result.json`, `report.junit`, and `build/build-env.json` contracts. A nonzero Xcode exit remains a failure even if the xcresult says passed. Missing or inconsistent evidence fails. Stale JUnit is removed before execution.

The archive command writes `build/application.ipa` and `build/rust-archive.json`. Its report is local archive evidence, explicitly **not an authenticated release candidate**. It does not import signing assets, mint provenance, upload binaries, or mutate App Store Connect. Run it in a dedicated app checkout to avoid overlapping other build output.

## Screenshot capture

The scheme and target must be explicit; the app continues to own its Swift screenshot scenarios. Existing SnapshotHelper cache files are supported without loading Fastlane.

```sh
/path/to/platform/rust/target/release/ios-release --app-root /path/to/app screenshots \
  --scheme PicStripScreenshots --test-target PicStripUITests \
  --only-testing PicStripUITests/PicStripUITests/testAllScreenshots \
  --devices 'iPhone 18 Pro Max' --languages en-US,ar-SA
```

Devices and locales must be members of the app configuration. `--photo` and `--video` can seed app-relative fixture files. Capture uses a unique job directory and compiled test products. Raw output defaults to `build/rust-screenshots/images`; custom output must also remain below `build`. Existing captures move into the new job's evidence directory. New images become visible only after all requested captures and configured PNG dimensions pass. App-owned source screenshots are never overwritten by this command.

Existing SnapshotHelper versions read the real host's `Library/Caches/tools.fastlane` directory; XCTest overwrites `SIMULATOR_HOST_HOME`, so that variable cannot redirect their cache. The default bridge therefore requires a disposable GitHub-hosted runner. It atomically claims an absent cache directory, refuses any existing cache, and moves its owned files into the job evidence on exit. It never touches a developer's existing screenshot cache.

Apps whose helper explicitly reads `IOS_RELEASE_SNAPSHOT_HOME` can opt into `--isolated-cache`, including for local capture. The helper should use that environment variable before falling back to `SIMULATOR_HOST_HOME`; the corresponding `Library/Caches/tools.fastlane` directory then lives inside the job's app-local home. PicStrip's current helper continues to use the disposable-host bridge without source changes.

One recovery is allowed per capture job, only on disposable GitHub-hosted runners and only for the recorded pre-test XCTest bootstrap signature. Recovery resets the exact resolved simulator UUID and reuses the same compiled test run. Assertion failures, app launch failures, executed tests and unknown evidence never trigger recovery. First-attempt logs, xcresults, readiness and recovery records are retained under `build/rust-screenshots/<job>`.

Screenshot composition remains app-owned. Native capture outputs opaque RGB PNGs suitable for Apple asset delivery. The native store commands upload these without loading Fastlane.

## Validation and adoption

The additive `Rust CLI checks` workflow tests Linux and macOS binaries, compares XCTest/localization contracts with the deployed Python implementation, and exercises an immutable PicStrip source checkout. Each native test runtime and each screenshot device runs on its own fresh runner. English and Arabic capture test both ordinary and right-to-left locales. These jobs do not receive release secrets or production approval permissions.

Before changing consumer defaults, require passing existing platform regressions and native canaries, full configured screenshot coverage, and a fresh signed archive comparison against the current release adapter. Native signing, Apple operations, attested selection and recovery need live adoption checks beyond simulated API contracts. A passing unsigned canary does not authorize a production release.

To develop the preview:

```sh
cd rust
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
python3 tests/parity.py target/release/ios-release
```
