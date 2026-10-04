# Native iOS release workflows

Build once, verify the candidate, and deliberately promote the same IPA to TestFlight and the App Store. Each consumer keeps its own accounts, credentials, signing assets, environments and approvals.

| Your task | Start here |
| --- | --- |
| Start a native iPhone/iPad app with the Rust CLI | [Native quickstart](docs/native-quickstart.md) |
| Adopt the established release platform | [Setup and configuration contract](docs/setup.md) |
| Release or recover an interrupted operation | [Operations](docs/operations.md) |
| Understand security and ownership | [Architecture](docs/architecture.md) |
| Find exact workflow inputs, outputs, secrets or commands | [Generated reference](docs/reference.md) · [JSON](docs/reference.json) |
| Change shared code or documentation | [Maintenance](docs/maintenance.md) |
| Evaluate the standalone Rust build CLI | [Rust CLI preview](docs/rust-cli.md) |

```mermaid
flowchart LR
    App[App source and policy] --> Prepare[Parallel archive and QA]
    Prepare --> Candidate[Verified candidate]
    Candidate --> TestFlight[Explicit TestFlight promotion]
    TestFlight --> Accept[Device acceptance]
    Accept --> Publish[Immutable release]
    Publish --> Stage[App Store staging]
    Stage --> Approval[Production approval]
    Approval --> Review[Apple review]
```

## Consumer boundary

App repositories keep their release operating guide, caller workflows, configuration, metadata and screenshots. This repository owns reusable implementation and its documentation. Pin callers and tools to one full reviewed commit. Link platform docs at that same revision so instructions and implementation stay aligned.

The Python package exposes one `ios-release` interface for setup, diagnostics, checks, documentation and builds. Native analysis/tests use `xcodebuild` and validated xcresults. Fastlane remains behind the Apple adapter for signing, archive/export, screenshot capture and delivery. Python and Ruby dependencies are locked here; callers require no Node installation or npm manifest.

The opt-in Rust CLI now also discovers apps, manages encrypted signing assets and renewable profiles, prepares verified releases, and handles native Apple upload, staging, review and production management. Its reusable Actions keep app compilation, Apple API credentials and production approval separate. Existing consumer workflows retain their established path while signed and live delivery parity are verified.

## Work on the platform

```sh
brew install uv # macOS; install uv through your package manager on other systems
make setup
make docs
make check
```

Generation is local, deterministic and checked in CI. [Maintenance](docs/maintenance.md) covers the full regression suite and consumer verification required for behavior changes.
