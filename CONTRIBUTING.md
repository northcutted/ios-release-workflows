# Contributing

Thanks for helping make native iOS releases easier to maintain. Start with the
[documentation](docs/README.md) and [beta status](docs/native-status.md) to understand
which interfaces and adoption gates exist.

## Repository map

| Path | Purpose |
| --- | --- |
| `rust/` | Native executable, locked dependencies, command/API/signing contracts |
| `actions/native/` | Install a verified native package or compile an exact source pin |
| `.github/workflows/native-*.yml` | Native app lifecycle, package build, adoption/caller rehearsals |
| `examples/OrbitNotes/` | Small unsigned native app with extension, unit tests, screenshot helper |
| `docs/` | Consumer guides and checked-in workflow reference |
| `src/ios_release/`, `bin/ios-release` | Compatibility Python CLI and its source launcher |
| `scripts/ci/`, `fastlane/`, `actions/bootstrap/`, `actions/ruby/` | Existing platform implementation and Apple adapters |

Public integration points are the CLI, app configuration, and reusable workflow
inputs/outputs. Prefer those to adding consumer calls into internal scripts.
Compatibility paths remain in place for apps already pinned to them.

## Native edit loop

Install Rust; `rust/rust-toolchain.toml` selects the reviewed toolchain.

```sh
make native-check
make native-build
./rust/target/release/ios-release --help
```

`native-check` runs formatting, Clippy with warnings denied, and native contract
regressions. Build/test execution needs macOS/Xcode, but most CLI contracts also
run on Linux. Tests exercise mock Apple responses and local cryptographic fixtures;
they do not constitute live Apple adoption proof.

For an app-facing behavior change, run the relevant hosted adoption/consumer
checks against the exact commit. Every screenshot device/test runtime uses a fresh
runner. Retain first-attempt simulator evidence; assertion failures remain failures.

## Documentation and compatibility changes

Install [uv](https://docs.astral.sh/uv/), then:

```sh
make setup
make docs
make check-docs
make check
```

Documentation generation is deterministic and offline. The profile in
`.github/ios-release-docs.json` includes consumer guides, contributor instructions,
and example READMEs in local link checks. For Ruby adapter changes, run the locked
Apple contract suite described in [maintenance](docs/maintenance.md#edit-loop).

Update focused prose for changed behavior and regenerate workflow reference files.
The generated command table describes the compatibility CLI; native commands have
[their own reference](docs/native-commands.md) and `--help`.

## Proposing a change

Describe the concrete behavior, why it matters, and the checks run. Keep app-owned
policy, compliance answers, metadata, and screenshot scenarios in the app. Preserve
existing config/workflow interfaces, exact producer pins, and approval boundaries.

Use [bug reports or adoption feedback](https://github.com/northcutted/ios-release-workflows/issues/new/choose)
for reproducible problems. Include CLI/source version, OS/Xcode, and redacted error
output. Never include API keys, signing passwords, P12 files, private keys, or
credential-bearing upload URLs.
