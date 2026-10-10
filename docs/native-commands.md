# Native CLI reference

[Documentation](README.md) · [Quickstart](native-quickstart.md)

Use `ios-release --help` and `ios-release COMMAND --help` for exact options.
`--commands-json` exposes the native command summary. This guide covers the Rust
binary; the [generated platform reference](reference.md) includes the separate
compatibility CLI.

## Select an app and inspect a plan

```sh
ios-release --app-root /path/to/app qa test
ios-release --app-root /path/to/app --plan archive --version 1.2.3 --build-number 42
```

`--config` accepts an app-relative or absolute configuration path;
`IOS_RELEASE_CONFIG` is also supported. The default is `.github/ios-release.json`
when that file exists; otherwise the CLI uses `.ios-release.json`. Pass `--config .ios-release.json` when both exist.
`--plan` prints an operation plan without launching tools or changing files.
Arguments pass directly to subprocesses; they are not evaluated as shell code.

## Commands

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
| `init` | Discovers an app on first setup; existing configurations only update team/tracking/encryption declarations |
| `auth login`, `auth status` | Saves the Apple API key privately or inspects its public identity |
| `signing sync`, `signing import`, `signing status` | Reconciles owned certificates/profiles, imports existing identities and reports renewal state |
| `qa all`, `release prepare` | Runs configured QA and prepares a source-bound signed release |
| `github setup`, `github fetch` | Configures protected native environments or downloads an authenticated preparation and recovery receipt |
| `store upload`, `store wait`, `store testflight` | Transfers the exact IPA, reconciles Apple processing and assigns beta groups |
| `store stage` | Selects the exact processed build and applies its prepared release policy |
| `store metadata` | Applies metadata/screenshots to the selected version; Actions stage runs both commands |
| `store submit --confirm` | Requests App Review for the selected version |
| `store validate`, `store beta-groups --create …` | Checks local store content before writes and configures an app-owned beta group by name |
| `store publish --confirm`, `store phased …`, `status` | Publishes an approved version, manages phased updates and reads production state |

QA keeps the existing `qa-results/<check>/result.json`, `report.junit`, and `build/build-env.json` contracts. A nonzero Xcode exit remains a failure even if the xcresult says passed. Missing or inconsistent evidence fails. Stale JUnit is removed before execution.

The archive command writes `build/application.ipa` and `build/rust-archive.json`. Its report is local archive evidence, explicitly **not an authenticated release candidate**. It does not import signing assets, mint provenance, upload binaries, or mutate App Store Connect. Run it in a dedicated app checkout to avoid overlapping other build output.
