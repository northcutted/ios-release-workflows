# Troubleshooting

[Documentation](README.md) · [Install](install.md) · [Configuration](native-configuration.md)

Start with the exact failing command and its retained output. For an app configured
with the native CLI:

```sh
ios-release doctor
ios-release --config .ios-release.json doctor --xcode
ios-release --config .ios-release.json --plan release prepare --version 1.2.3
```

`doctor` reports CLI/source identity and requirements. `doctor --xcode` validates
the configured toolchain and device; it needs an app configuration. `--plan` shows
the intended operation without running it. Neither diagnostic grants Apple or
GitHub access.

## Installation or the wrong command

- **The installer refuses an existing command:** use a separate install prefix
  and invoke that binary by its full path. Check `command -v ios-release` and
  `ios-release doctor` to identify the command your shell selects.
- **The package source does not match the workflow pin:** use a package and full
  source commit from the same published release. The
  [installation guide](install.md) records the current pair.
- **The beta reports `0.2.0` rather than `0.2.0-beta.1`:** this is the published
  beta's Cargo version display. Its source is identified by `platform_revision`;
  [install](install.md#existing-installations-and-upgrades) explains the distinction.

## App discovery, Xcode, and tests

- **Several projects/schemes are found:** select `--project` or `--workspace` and
  `--scheme` explicitly during first initialization. In Xcode, mark the scheme as
  shared and include its unit test target.
- **A new test target is missing:** add it to the shared scheme and the existing
  configuration's `test_targets`. Rerunning `init` on an existing configuration
  does not rediscover targets.
- **Xcode/SDK/runtime mismatch or unavailable simulator:** compare all configured
  Xcode fields with the installed tools, then install the required runtime/device
  or deliberately update the configuration. Your selected hosted runner also
  needs that exact toolchain. Resolve names with
  `ios-release --config .ios-release.json toolchain`.
- **Tests or screenshot capture fail:** inspect `qa-results/<check>/output.log`,
  `build/test_output/`, or `build/rust-screenshots/<job>/`. Preserve the first
  xcresult, command log, and readiness/recovery record. Assertion failures require
  an app/test fix. A known pre-test bootstrap failure can recover once on a fresh
  disposable hosted runner; [capture recovery](native-screenshots.md) documents
  the narrow automatic recovery rule. Report a timed-out result export with its
  diagnostics, even if XCTest printed that its tests passed.

## Signing and Apple access

- **Missing Apple credentials:** run `auth login` with your team API key and its
  downloaded `.p8` file, then inspect the public identity with `auth status`.
- **Team, privacy, encryption, or profile declarations are incomplete:** review
  every app/extension target in `.ios-release.json`, its privacy manifest and
  Info.plist, then follow [signing setup](native-signing.md).
- **The vault cannot be unlocked:** use its original password and encrypted vault
  backup. `github setup` checks the password before changing secrets. Reuse/import
  an existing identity only when its private key is available.
- **A capability cannot be provisioned:** configure the app's groups, iCloud
  containers, or Apple Pay associations in Apple Developer, then synchronize and
  review the profiles for all targets.

## Actions and delivery

- **Missing GitHub repository or a missing caller:** set `repository` in the
  reviewed native configuration and follow
  [configuration](native-configuration.md#review-after-initialization). A local-only
  `init` does not later generate its workflow on an existing configuration.
- **Repository controls or production reviewer checks fail:** use protected `main`
  as the default branch and a plan that supports the required
  [attestation and environment features](native-status.md#github-actions), then
  run `github setup`. The production gate is verified before release operations.
- **Release jobs skip on a PR:** this is expected; PRs run unsigned QA. Dispatch
  preparation/delivery operations from protected main after merging the caller.
- **Store content fails validation:** replace starter descriptions, fill required
  URLs/text for every configured locale, and provide supported opaque PNGs.
  `store validate` performs these checks locally before Apple writes.
- **An upload or review request stops after an uncertain response:** preserve the
  preparation and `operation.json`, then resume the same operation with its
  original CLI and preparation identity. Commands reconcile Apple state using
  that receipt. For Actions, retry the failed operation job in its original run;
  use `github fetch --run PREPARATION_RUN_ID` to retrieve authenticated inputs and
  recovery receipts for local management.

If the problem remains, [open a bug report](https://github.com/northcutted/ios-release-workflows/issues/new/choose)
with the package/source revision, OS/Xcode, exact command, and redacted error or
public run URL. Exclude private keys, signing passwords, and upload capability URLs.
