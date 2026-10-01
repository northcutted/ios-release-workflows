# Maintain the platform

[Start here](../README.md) · [Setup](setup.md) · [Operations](operations.md) · [Architecture](architecture.md) · [Reference](reference.md) · [Maintenance](maintenance.md)

## Edit loop

```sh
make setup
make docs
make check
# Also exercise Apple adapter contracts:
python3 bin/ios-release setup --apple
python3 bin/ios-release check --apple
```

`check` runs workflow policy, documentation/link checks and Python regression tests; `--syntax` also runs actionlint. `--apple` adds the locked Ruby contract tests. Linux and macOS CI exercise the contracts and real verifier installation. Changes to native QA additionally need a consumer run on both configured Xcode runtimes; mocked commands and recorded xcresults cannot prove a fresh build.

## Documentation generation

`ios-release docs` reads `.github/ios-release-docs.json`, YAML interfaces, selected app configuration, and the platform command registry. It generates Markdown and JSON with stable ordering and no network or timestamps. `--check` is read-only and rejects stale output or broken simple inline local links/heading anchors in the configured pages. It does not fetch external links, execute examples, or validate historical evidence.

The platform profile generates reusable workflow/command contracts. A consumer profile generates its own effective workflow reference. Consumer examples optionally call its classifier with `examples JSON_PATH_LIST`; that app code executes only in local or unprivileged documentation jobs. It never authorizes a release. Secret values and raw workflow scripts are excluded.

Generated JSON has `schema_version: 2`. It adds reusable outputs/secrets and profile/command data; consumers should query named fields. Changes to existing meanings require another schema bump. The generator belongs here; app-specific examples, page navigation and checked-in generated results belong in the consumer. Keep intent and recovery in focused prose, configuration facts in source and live state in service readback.

## Agent recipe

1. Read the relevant task page, then query `docs/reference.json` for the specific command/workflow.
2. Edit shared behavior here; app policy and content stay in the consumer.
3. Regenerate docs and run the affected regression tests plus workflow policy checks.
4. Verify a consumer against the exact new commit before proposing a pin update. Preserve approved older producers needed for retained builds.
5. Report local, hosted, simulator and signed-release evidence separately.

## Supported command interface

Call `python3 /path/to/platform/bin/ios-release --help`, or inspect `--commands-json`. Global `--app-root` and `--config` select the consumer. Command arguments follow the command name; `qa --help` and other subcommand help describe them. Prefer this surface to internal `scripts/ci` paths. Reusable workflow inputs and this CLI are the supported integration points.

Local consumers fetch the full reviewed commit with `setup`, which synchronizes the Python environment from `uv.lock`; subsequent commands reuse it. The source launcher needs uv. The bootstrap Action installs pinned uv and Python, synchronizes the same lock and exposes `ios-release` on PATH. `setup --apple` installs locked gems using the exact Ruby version; it can install that version with mise locally without changing the active Ruby. `doctor --apple --xcode` reports missing tools or mismatches. Xcode must already be installed.

The installable wheel includes platform scripts, the Ruby adapter and its locks. App-owned screenshot scenarios and composition stay in the app. `setup --images` prepares its hash-locked requirements and `screenshots-compose` uses that isolated interpreter. `IOS_RELEASE_ROOT` is a trusted override set by the pinned bootstrap Action or deliberately for local platform development; it must never come from downloaded release evidence. The CLI does not grant GitHub/Apple authority; mutation commands retain their existing environment, authentication and approval requirements.

## Dependency compatibility

Python dependencies are pinned in `pyproject.toml` and hash-locked in `uv.lock`. Version policy lives in `.github/ios-version.json`. The parser supports Conventional Commit headers, breaking notes, reverts, stable reachable tags and explicit release rules. Legacy `.releaserc.json` is accepted only for the supported Conventional Commits options; unsupported plugins/options fail before a candidate is prepared.

The checked-in reference fixture captures version decisions and exact release-note text from the former locked Node analyzer. Compare new behavior against it before modifying version semantics. Linux and macOS CI also build and install the wheel outside the checkout.

Minitest 6 extracted mocks into [minitest-mock](https://github.com/minitest/minitest-mock); contract tests declare and require that dependency explicitly. Ruby dependencies remain locked with checksums.

## Read-only repository control verification

GitHub [redacts REST ruleset bypass actors](https://docs.github.com/en/rest/repos/rules#get-a-repository-ruleset) from tokens that cannot edit rulesets. Keep the publisher at administration read. In an owner-authenticated shell, after applying the repository protections, run:

```sh
IOS_RELEASE_CONFIG=/path/to/app/.github/ios-release.json \
  python3 bin/ios-release controls-capture --publisher-app-id YOUR_APP_ID --output /tmp/controls.json
```

Copy the resulting object into the app configuration's `github_controls` field and review it through the required PR. Capture compares owner-visible REST bypass actors with complete GraphQL actor nodes and checks that the ruleset did not change during capture. The baseline binds the publisher App ID, ruleset IDs, actor-node IDs and server-controlled modification timestamps. It contains no credentials.

Promotion checks all public protections live. Bootstrap freezes `github_controls` from protected main before loading archived app configuration. If REST bypass details are hidden, verification requires exact baseline IDs and modification instants (including fractional seconds, independent of timezone formatting), plus complete GraphQL counts and identities.

GitHub can redact a private Integration node itself as `[null]` even for that App's administration-read installation token. The only accepted redacted shape is one node, count one, no next page, exactly one owner-recorded Integration with `always` bypass, unchanged server-controlled ruleset IDs/time, and live `current_user_can_bypass: always`. The recorded App must still equal the configured publisher. This combines the owner-verified identity with an unchanged ruleset and the current token's effective permission; it does not claim the hidden identity was read directly. Empty, truncated, inaccessible, conflicting or changed evidence fails closed. Any ruleset edit requires owner inspection and a reviewed baseline update, even if benign. Never refresh the baseline automatically in a privileged release job or grant the publisher administration write merely to read its own controls.

## Verifier installation

`actions/slsa-verifier` keeps the upstream installer on supported Linux x64 runners. Its [upstream installer is Linux-only](https://github.com/slsa-framework/slsa-verifier/blob/v2.7.1/actions/installer/README.md). On macOS arm64/x64, the platform downloads official v2.7.1 assets using reviewed SHA256 pins for both the executable and provenance. Both hashes must pass before execution; the pinned bootstrap then verifies the expected upstream source/tag before entering PATH. The macOS assets were independently provenance-verified before pinning. Unsupported platforms fail explicitly, and platform CI exercises actual installation on Linux and macOS. This does not remove IPA provenance verification from the upload job.
