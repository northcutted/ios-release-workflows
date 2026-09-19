# Native iOS release workflows

Build once, authenticate the candidate, and promote that exact IPA to App Store Connect. Consumers retain their own Apple account, GitHub repositories, signing assets, environments, and approvals. The platform receives no customer credentials outside the customer's workflow run.

## Support

Version 1 supports native Xcode projects/workspaces, multiple app extensions, explicit runtime dependency inventories, GitHub-hosted runners, and existing App Store app records. Public repositories and GitHub Enterprise Cloud private repositories are supported. GitHub Enterprise Server, private repositories without the required GitHub attestation/approval features, Flutter, and React Native are outside v1.

Each Apple app must have one owning release repository: GitHub concurrency locks do not span repositories. Pin both `uses:` and `platform_revision` to the **same full commit**. The isolated SLSA generator's supported version tag is the sole external reference exception.

## Consumer setup

1. Copy `examples/minimal.json` or `examples/extensions.json` to `.github/ios-release.json`. Replace every example identity and explicitly declare encryption, tracking, entitlements, signing profiles, and App Store policy. `examples/picstrip.json` shows the first production consumer. Do not copy another app's compliance answers.
2. Commit reviewed metadata/screenshots and a Conventional Commits `.releaserc.json`. Configure the exact installed Xcode/SDK/runtime and simulator names. `build_number_offset` must preserve monotonic build numbers when migrating an existing preparation workflow.
3. Create protected environments: `signing`, `testflight`, `release-publishing`, `app-store-staging`, `production`, and `app-store-observe`. Keep signing and promotion environments on main, staging/production on `v*` tags, and production approval mandatory with administrator bypass disabled.
4. Put signing secrets only in `signing`; App Store keys in the environments that need them; publisher App credentials only in `release-publishing`. Do not use `secrets: inherit`. Bind only the named secrets declared by each release interface (`NAME: ${{ secrets.NAME }}`); repository copies are unnecessary. GitHub needs these explicit bindings even when the protected job environment supplies the value. CI declares and receives no release secrets. Use narrowly scoped, consumer-owned Apple keys and publisher Apps.
5. Require PRs and the caller's always-reporting `CI Gate`, restrict `v*` tag creation/update/deletion to the publisher App, and enable immutable releases. The publisher App needs repository contents write and administration read. Keep `RELEASE_DISTRIBUTION_ENABLED` unset/false until rollout passes.

## Interfaces

| Workflow | Inputs in addition to `config` and `platform_revision` | Result |
| --- | --- | --- |
| `ci.yml` | exact `source`, unique `prefix`, `test_workers` (default 1) | secret-free QA and always-reporting gate |
| `prepare.yml` | none; caller must run on main | candidate artifact ID and SHA256; no upload |
| `promote.yml` | exact `artifact_id`, `sha256`, explicit `upload_adapter`, `publish` (default false), optional `processed_artifact_id` and `processed_sha256` | verified TestFlight build; optionally immutable release |
| `deploy.yml` | `release_tag`, optional exact `metadata_commit`, `submit` | staged build, production approval, confirmed submission and signed receipts |
| `observe.yml` | none | read-only status for recent immutable releases |

Call `ci` from `pull_request`, `prepare` from main pushes/manual dispatch, `promote` only from manual dispatch on main, `deploy` from `release.published` or manual dispatch **on that release tag**, and `observe` hourly on main. Use a caller `CI Gate` job with `if: always()` and reject any failed/skipped mandatory job. Required caller permissions for prepare are contents write, actions read, id-token write, attestations write and artifact-metadata write; the platform narrows permissions separately for compilation and each evidence job. CI requires contents read only. Promotion/deployment callers need actions read, contents read and attestation signing permissions; the publisher App alone creates releases.

Consumers need only configuration, caller workflows, and their app sources/assets. CI and release tools run from this pinned repository through GitHub's `$/` self-repository references. Privileged jobs never execute consumer Fastfiles, Gemfiles, or shell hooks. App build phases execute only in the app's own build/test jobs; signing jobs necessarily make that app's signing material available to its approved compilation.

## Release and recovery

Preparation produces schema-v3 evidence binding source, platform revision, app/team, configuration digest, QA (including localization), IPA/SBOMs, and run identity. Promotion verifies native attestations and isolated SLSA provenance, checks main ancestry, then rechecks the IPA immediately before upload. Artifacts expire after 90 days; expired candidates are not guessed or rebuilt during deployment.

`TESTFLIGHT_CANARY_ENABLED=true` allows manual TestFlight rehearsal without publication. `RELEASE_DISTRIBUTION_ENABLED=true` additionally permits publication/staging/submission. `publish: false` remains the promotion default. The default upload adapter is Transporter until a consumer validates the Build Uploads adapter in a real canary. Set the adapter in the candidate configuration and promotion input consistently; no automatic fallback exists.

The Build Uploads adapter reserves an Apple upload/file, transfers bounded byte ranges, commits SHA256, and binds Apple's processed build through the upload relationship. Failed attempts retain operation receipts. Rerun failed jobs in the **same promotion run** to recover prior successful transfer receipts. Existing uploads are reused only with a matching SHA256 or that run's verified successful Transporter receipt. Every successful canary also emits a signed final handoff artifact. To publish it later, supply the original candidate ID/digest plus that exact processed artifact ID/digest; the platform authenticates both, reads back the recorded Apple build, and performs no transfer. An ambiguous legacy upload without proof is rejected. Conflicting immutable releases are never replaced.

Staging explicitly attaches the processed build and rejects replacement of an already selected different build. Production approval precedes an explicit release-policy update and readback. Submission preserves permitted metadata edits, resumes only matching review items, and records success only after Apple's submitted state is visible. Receipts are separately attested workflow artifacts with 90-day retention; they do not mutate immutable releases and omit review credentials/contact details.

Metadata-only updates require an exact commit reachable from protected main. The operation records each consumed text file's hash and uses the same production submission gate. The observer changes no Apple state. Apple review outcomes, agreements/account setup, and owner-supplied compliance facts remain explicit responsibilities; the platform does not invent them.

## Security and validation

The target is SLSA Build L3 for the GitHub-produced IPA, not Apple's redistributed binary. The isolated generator protects provenance keys from app compilation; native attestations additionally bind evidence and receipts. Source control, environment approvals, producer pinning, artifact verification, immutable publication, and consumer-owned credentials are separate required controls. Passing provenance verification alone is not certification of the complete deployment.

Run `npm ci --ignore-scripts`, install the locked Ruby gems, then `npm run check:workflows` and `npm run test:ci`. The actionlint adapter validates `$/` targets before normalizing their spelling for actionlint 1.7.12, which predates that documented GitHub syntax. No source files are rewritten by linting.

Before enabling distribution: run complete native iOS QA, a signed candidate rehearsal, a TestFlight canary and draft inspection. Compare five equivalent runs before enabling two XCTest workers; require reliability and at least 15% median improvement. Local fixture tests do not substitute for a customer-owned external-repository or App Store rehearsal.

## Dependency compatibility

Dependabot groups release-analysis packages because parser and preset major versions must remain compatible. Regression tests cover version rules and rendered release notes.

The notes generator 14.1.1 requests writer 8, while the Conventional Commits 10.4.0 preset requires writer 9. A scoped npm override pins `conventional-changelog-writer` to 9.2.1, whose `writeChangelogString` export is compatible with the generator. Remove this override when the generator supports writer 9, with the regression suite passing. See the [preset release notes](https://github.com/conventional-changelog/conventional-changelog/releases/tag/conventional-changelog-conventionalcommits-v10.4.0).

Minitest 6 extracted mocks into [minitest-mock](https://github.com/minitest/minitest-mock); contract tests declare and require that dependency explicitly. Ruby dependencies remain locked with checksums.
