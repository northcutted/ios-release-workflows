# Native beta status

[Documentation](README.md) · [Quickstart](native-quickstart.md) · [Migration](from-fastlane.md)

`ios-release` is a focused alternative for native iPhone/iPad release automation.
The current published package is **0.2.0-beta.1**. A beta is suitable for an
adoption rehearsal; production replacement requires the remaining live checks.

## Supported scope

| Area | Scope |
| --- | --- |
| Apps | Native Xcode projects/workspaces, shared schemes, unit tests, app extensions |
| Build hosts | macOS with the exact configured Xcode version/build, SDK, and simulator runtime |
| Binaries | macOS arm64/x86_64; Linux x86_64 for API and release management |
| Signing | Apple distribution identities, app-specific App Store profiles, encrypted local/repository vault |
| Delivery | Existing App Store Connect app records; TestFlight, metadata/screenshots, review, manual/scheduled/automatic release policy, phased updates |
| CI | Generated GitHub-hosted Actions; local CLI also available |
| Customization | Declarative app configuration and app-owned screenshot tests/content |

Android, Flutter/React Native onboarding, macOS distribution, Windows binaries,
Fastlane plugins/custom Ruby lanes, and GitHub Enterprise Server are outside the
current native scope. App groups, iCloud containers, and Apple Pay associations
must already be configured with Apple. App record creation, account agreements,
privacy, age rating, pricing, and availability remain App Store Connect steps.

## GitHub Actions

The native release path requires protected main, artifact attestations, and
required production environment reviewers with administrator bypass disabled.
Public repositories can use the relevant features; private repositories need
GitHub Enterprise Cloud for
[artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations).
Your repository also needs
[required environment reviewers](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments).
`github setup` verifies the production gate rather than assuming it exists.

Local CLI operations do not require a GitHub attestation plan feature. Local
preparation creates release evidence, but does not mint GitHub build provenance.
Release artifacts and receipts have 90-day Actions retention; preserve them for
longer-term management.

## Recorded beta validation

The following evidence is from **October 4, 2026**; these are fixed runs, not a
claim about every future app, Apple API change, or simulator run.

| Capability | Evidence | What it establishes |
| --- | --- | --- |
| Native packaging | [Package run](https://github.com/northcutted/ios-release-workflows/actions/runs/37226814938) and [published assets](https://github.com/northcutted/ios-release-workflows/releases/tag/native-v0.2.0-beta.1) | Three tested binaries, SHA256 checksums, hosted build provenance; installer verified in a separate prefix |
| Second-app onboarding | [Orbit Notes rehearsal](https://github.com/northcutted/ios-release-workflows/actions/runs/37220458472) | Discovery, initialization, unit tests, analysis, localization, and iPhone/iPad English/Arabic screenshots |
| Reusable Actions | [Caller rehearsal](https://github.com/northcutted/ios-release-workflows/actions/runs/37220458738) | An app caller using pinned native workflows; privileged PR operations skip |
| Native regressions and consumer QA | [Rust checks](https://github.com/northcutted/ios-release-workflows/actions/runs/37220458474) | Linux/macOS contracts, immutable PicStrip QA, screenshots, and evidence compatibility |
| Signed archive/export | [PicStrip diagnostic canary](https://github.com/northcutted/picstrip/actions/runs/37228306247) | Signed main app and extension, matching dSYMs, independent IPA verification, and read-only Apple status |

The signed canary used existing signing assets. Its diagnostic IPA was never
uploaded or promoted. It proves archive/export and independent verification,
rather than native certificate issuance or Apple delivery.

## Remaining adoption gates

- Live certificate creation and renewal using the native vault.
- Live native upload, processing, TestFlight assignment, store staging, review,
  release, and recovery with consumer-owned Apple credentials.
- Full configured screenshot/locale coverage for each migrating app.
- Measured setup/build duration and repeated simulator results before claiming a
  performance advantage or lower flake rate.

These paths are implemented and have local/contract coverage. Until their live
checks pass, existing production consumers keep their reviewed default workflows.
[Migration](from-fastlane.md) describes how to collect that proof incrementally.
