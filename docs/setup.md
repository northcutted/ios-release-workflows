# Adopt the platform

This guide covers the existing Python/Ruby compatibility platform. For the native
Rust CLI, start with the [native quickstart](native-quickstart.md) or [documentation index](README.md).

[Start here](../README.md) · [Setup](setup.md) · [Operations](operations.md) · [Architecture](architecture.md) · [Reference](reference.md) · [Maintenance](maintenance.md)

## Support

Version 1 supports native Xcode projects/workspaces, multiple app extensions, explicit runtime dependency inventories, GitHub-hosted runners, and existing App Store app records. Public repositories and GitHub Enterprise Cloud private repositories are supported. GitHub Enterprise Server, private repositories without the required GitHub attestation/approval features, Flutter, and React Native are outside v1.

Each Apple app must have one owning release repository: GitHub concurrency locks do not span repositories. Pin both `uses:` and `platform_revision` to the **same full commit**. The isolated SLSA generator's supported version tag is the sole external reference exception.

## Consumer setup

1. Copy [examples/minimal.json](../examples/minimal.json) or [examples/extensions.json](../examples/extensions.json) to `.github/ios-release.json`. Replace every example identity and explicitly declare encryption, tracking, entitlements, signing profiles, and App Store policy. [examples/picstrip.json](../examples/picstrip.json) shows the first production consumer. Do not copy another app's compliance answers.
2. Commit reviewed metadata/screenshots and a Conventional Commits [version policy](../.github/ios-version.json) at `.github/ios-version.json`. Configure the exact installed Xcode/SDK/runtime and simulator names. `build_number_offset` must preserve monotonic build numbers when migrating an existing preparation workflow.
3. Create protected environments: `signing`, `testflight`, `release-publishing`, `app-store-staging`, `production`, and `app-store-observe`. Keep signing and promotion environments on main, staging/production on `v*` tags, and production approval mandatory with administrator bypass disabled.
4. Put signing secrets only in `signing`; App Store keys in the environments that need them; publisher App credentials only in `release-publishing`. Do not use `secrets: inherit`. Bind only the named secrets declared by each release interface (`NAME: ${{ secrets.NAME }}`); repository copies are unnecessary. GitHub needs these explicit bindings even when the protected job environment supplies the value. CI declares and receives no release secrets. Use narrowly scoped, consumer-owned Apple keys and publisher Apps.
5. Require PRs and the caller's always-reporting `CI Gate`, restrict `v*` tag creation/update/deletion to the publisher App, and enable immutable releases. The publisher App needs repository contents write and administration read. Capture the owner-verified controls [baseline](maintenance.md#read-only-repository-control-verification) before promotion. Keep `RELEASE_DISTRIBUTION_ENABLED` unset/false until rollout passes.

## Workflow integration

| Workflow | Inputs in addition to `config` and `platform_revision` | Result |
| --- | --- | --- |
| `ci.yml` | exact `source`, unique `prefix`, `test_workers` (default 1) | secret-free QA and always-reporting gate |
| `prepare.yml` | none; caller must run on main | candidate artifact ID and SHA256; no upload |
| `release.yml` | explicit `action`, selected run URL/ID or release-tag `source`, optional exact `metadata_commit` | resolves verified inputs and orchestrates promotion or protected metadata deployment |
| `promote.yml` | exact `artifact_id`, `sha256`, explicit `upload_adapter`, `publish` (default false), optional `processed_artifact_id` and `processed_sha256` | verified TestFlight build; optionally immutable release |
| `deploy.yml` | `release_tag`, optional exact `metadata_commit`, `submit` | staged build, production approval, confirmed submission and signed receipts |
| `observe.yml` | none | read-only status for recent immutable releases |

Call `ci` from `pull_request`, `prepare` from main pushes/manual dispatch, and `release` from explicit manual dispatch on main. Keep `deploy` on immutable release or protected operation tags. Schedule `observe` at a suitable baseline such as every six hours, with manual refresh available. A caller may classify changes conservatively, but its required `CI Gate` must always report and reject failed or unexpectedly skipped jobs. Required caller permissions for prepare are contents write, actions read, id-token write, attestations write and artifact-metadata write; compilation and evidence jobs narrow those permissions. CI requires contents read only. Release/promotion/deployment callers need actions read, contents read and attestation signing permissions; the publisher App alone creates releases and operation tags.

The Release actions are **Upload to TestFlight**, **Prepare App Store submission**, **Update store metadata**, and **Update metadata and request review**. Upload accepts an exact preparation run URL/API ID or `#preparation-number`. App Store preparation also accepts a successful TestFlight run URL, authenticates its processed handoff, and avoids another transfer. Metadata actions take an immutable release tag and an exact reviewed main-ancestor commit; a blank commit freezes the caller's main revision. There is no latest-candidate fallback. Failed runs, forked sources, ambiguous/expired artifacts, checksum mismatches, and unapproved producer signatures stop selection. The lower-level `promote.yml` interface remains available for advanced integrations.

Published-release operations create a protected tag binding the release, originating run, metadata commit (or original assets), submission choice, and reviewed tool source: `RELEASE-op-RUN-stage|submit-METADATA|original-deploy-SOURCE`. Configure the consumer's deployment caller to handle its create event, call `ios-release deployment-ref verify`, and forward the resolved `release_tag`, `metadata_commit`, and `submit` outputs to `deploy.yml`. Retrying the request reuses this tag and does not emit a second deployment event. Retry failed Apple jobs in the original deployment run. Production retains its tag restriction and human approval.

Consumers need only configuration, caller workflows, and their app sources/assets. CI and release tools run from this pinned repository through GitHub's `$/` self-repository references. Privileged jobs never execute consumer Fastfiles, Gemfiles, or shell hooks. App build phases execute only in the app's own build/test jobs; signing jobs necessarily make that app's signing material available to its approved compilation.

## Configuration contract

The app configuration uses `schema_version: 1`; [configuration.py](../scripts/ci/configuration.py) is the executable validator. Start with [one app](../examples/minimal.json) or [multiple extensions](../examples/extensions.json). The [generated reference](reference.md) extracts workflow inputs, outputs, secrets, defaults and job dependencies from YAML.

The version 1 contract intentionally requires protected `main`, a `main.yml` preparation caller, the complete five-check QA set and a GitHub SSH signing repository. Runner labels remain platform-owned. Supporting another runner fleet or repository convention requires a reviewed platform change, rather than an unchecked consumer hook. Config fixtures cover unrelated apps; a new production consumer still needs its own signed rehearsal.
