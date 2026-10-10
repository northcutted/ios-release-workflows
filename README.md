# ios-release

**Native iOS releases, from your first build to your next production update.**

[![Native beta](https://img.shields.io/badge/native-v0.2.0--beta.1-blue)](https://github.com/northcutted/ios-release-workflows/releases/tag/native-v0.2.0-beta.1)
[![Rust checks](https://github.com/northcutted/ios-release-workflows/actions/workflows/rust-checks.yml/badge.svg?branch=main)](https://github.com/northcutted/ios-release-workflows/actions/workflows/rust-checks.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

One Rust executable and a generated GitHub Actions workflow for native iPhone and
iPad apps. Discover your Xcode project, run tests, manage signing, prepare a
verified IPA, and carry that same build through TestFlight and App Review.

```sh
ios-release init
ios-release qa all
# After connecting Apple access and signing in the quickstart:
ios-release release prepare --version 1.0.0
```

**Public beta.** Installation, native QA, screenshots, and signed archive/export
have hosted validation. Certificate issuance/renewal and Apple upload/submission
have contract coverage; they still need live adoption proof. See
[supported scope and validation](docs/native-status.md) before switching an
existing production app.

## Why choose it?

- **Less tooling to maintain.** The native binary needs no Ruby, Bundler, Python,
  Node, or Fastlane runtime. Xcode builds still use Apple's tools.
- **Start with your app.** `init` discovers shared schemes, test targets,
  extensions, and Xcode settings, then writes one app configuration and pinned
  Actions workflow.
- **Signing belongs to you.** Reuse an existing distribution identity or manage
  certificates and profiles in an encrypted vault committed with your app.
- **Prepare once, promote the same build.** QA, IPA verification, and provenance
  bind a release to its source and bytes. GitHub Actions separates compilation,
  Apple credentials, and production approval.
- **Keep managing shipped apps.** Read store status, update metadata, manage
  TestFlight groups and phased releases, and resume operations using retained
  release receipts.

The tradeoff is focus: native Xcode apps and a prescribed release flow.
Fastlane has a much broader ecosystem and customization surface. Start with
[the migration guide](docs/from-fastlane.md) if you depend on custom lanes or
plugins. No speed or long-term reliability advantage is claimed without measured
evidence.

## Install

On macOS, install [GitHub CLI](https://cli.github.com/) (`brew install gh`) and run
`gh auth login`. Download and run the installer from the published beta's exact
source revision:

```sh
task_installer="$(mktemp)"
curl -fsSL https://raw.githubusercontent.com/northcutted/ios-release-workflows/a3c32bcae61a8a174f3c5ea6a4ba0b623aaaf33a/scripts/install-native.sh -o "$task_installer"
bash "$task_installer" 0.2.0-beta.1 "$HOME/.local" a3c32bcae61a8a174f3c5ea6a4ba0b623aaaf33a
rm "$task_installer"
export PATH="$HOME/.local/bin:$PATH"
ios-release doctor
```

The installer checks the binary's SHA256 and GitHub build provenance before
executing it. It supports Apple silicon and Intel Macs, plus Linux x86_64 for
API/release management. Building and simulator tests require macOS, Xcode, and an
installed iOS runtime. Signing also uses OpenSSL and macOS Keychain tools.

Already have an `ios-release` command? Choose a separate install prefix and invoke
its binary explicitly. See [installation and upgrades](docs/install.md).

## Ship your first app

Follow the [quickstart](docs/native-quickstart.md): connect your app, check it
locally, set up Apple access and signing, then enable Actions. You keep your own
Apple account, GitHub repository, credentials, store content, and release approvals.
Apple's account setup and app declarations remain yours to complete.

| I want to… | Guide |
| --- | --- |
| Try it without an Apple account | [Orbit Notes example](examples/OrbitNotes/README.md) |
| Review my app settings | [Configuration](docs/native-configuration.md) |
| Set up certificates and profiles | [Signing](docs/native-signing.md) |
| Deliver or manage a release | [Releases and recovery](docs/native-releases.md) |
| Capture App Store screenshots | [Screenshots](docs/native-screenshots.md) |
| Find a command or option | [Native CLI reference](docs/native-commands.md) |
| Adopt it alongside Fastlane | [Migration](docs/from-fastlane.md) |
| Understand beta coverage and limits | [Status](docs/native-status.md) |

[All documentation](docs/README.md) · [Troubleshooting](docs/troubleshooting.md) ·
[Contributing](CONTRIBUTING.md) ·
[Report a bug](https://github.com/northcutted/ios-release-workflows/issues/new/choose)

## Existing platform consumers

This repository also maintains the Python/Ruby release platform used by existing
apps. Its entrypoints, workflow interfaces, configuration, and pins remain
supported. [Compatibility platform docs](docs/setup.md) describe that path;
native adoption is explicit and reversible.
