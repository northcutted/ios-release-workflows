# Documentation

`ios-release` is the native Rust CLI. Start with the quickstart; each subsequent
guide covers one part of shipping and maintaining an iPhone/iPad app.

| Guide | Purpose |
| --- | --- |
| [Install](install.md) | Install a verified binary, choose a prefix, or build from source |
| [Quickstart](native-quickstart.md) | Connect your app and get to your first release |
| [Configuration](native-configuration.md) | Review app settings, QA, store policy, and committed files |
| [Signing](native-signing.md) | Apple API access, encrypted vaults, existing certificates, renewal |
| [Releases and recovery](native-releases.md) | GitHub Actions, local delivery, store content, production updates |
| [Screenshots](native-screenshots.md) | App-owned UI tests, device/locale capture, simulator recovery |
| [CLI reference](native-commands.md) | Commands, configuration selection, planning, evidence paths |
| [From Fastlane](from-fastlane.md) | Rehearse adoption while retaining an existing release path |
| [Troubleshooting](troubleshooting.md) | Diagnose installation, Xcode, signing, and interrupted operations |
| [Beta status](native-status.md) | Supported platforms, validation evidence, and remaining work |
| [Orbit Notes](../examples/OrbitNotes/README.md) | Try an unsigned app with a share extension and screenshot tests |

## Contributing

[Contributor guide](../CONTRIBUTING.md) covers the repository layout and local
checks. [Maintenance](maintenance.md) covers shared contracts, documentation
generation, and compatibility adapter changes.

## Compatibility platform

Existing Python/Ruby consumers continue to use `.github/ios-release.json` and
their reviewed workflow pins. These guides describe that integration, rather
than the native `.ios-release.json` onboarding flow:

- [Setup](setup.md): configuration, consumer ownership, workflow integration.
- [Operations](operations.md): preparation, promotion, deployment, and recovery.
- [Architecture](architecture.md): trust boundaries and release ownership.
- [Generated reference](reference.md) ([JSON](reference.json)): checked-in workflow
  interfaces and **compatibility CLI** commands. The native command reference is
  maintained separately above.
