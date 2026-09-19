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
5. Require PRs and the caller's always-reporting `CI Gate`, restrict `v*` tag creation/update/deletion to the publisher App, and enable immutable releases. The publisher App needs repository contents write and administration read. Capture the owner-verified controls baseline below before promotion. Keep `RELEASE_DISTRIBUTION_ENABLED` unset/false until rollout passes.

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

To repair promotion tooling while reusing an existing candidate, update the pinned platform and explicitly add the candidate's reviewed producer commit to `trusted_producer_revisions` in the protected consumer configuration. This optional list accepts at most 20 full commit SHAs; the current platform commit is always included. Bootstrap captures this policy from the protected checkout before loading artifact configuration. An artifact cannot authorize its own producer. Each build and promotion signature must match its own recorded, approved commit and the fixed platform workflow identity; source, app/team, QA, SLSA and asset checks remain required. Also retain an approved prior promotion commit when reusing its processed handoff. Review each addition and remove historical approvals when their artifacts are no longer needed. This mechanism repairs deployment without rebuilding an already verified IPA.

GitHub loads a release-event caller from the original app tag. When promotion runs from a newer reviewed main commit, the publisher also creates `vVERSION-deploy-FULL_COMMIT` after immutable publication. Consumers must handle the `create` event for this protected tag and resolve the original release with `deployment_ref.py verify`. The tag uses the same publisher-only `v*` protection and staging/production environment rules. Its suffix must equal the event commit, and that commit must be on protected main. Deployment still authenticates the original immutable release and exact Apple build. An old release caller may reject a newer promotion signer; the deployment-tag run uses the corrected pinned tools. Retry by dispatching deployment on that exact deployment tag. Neither application tags nor published assets are moved.

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

## Read-only repository control verification

GitHub [redacts REST ruleset bypass actors](https://docs.github.com/en/rest/repos/rules#get-a-repository-ruleset) from tokens that cannot edit rulesets. Keep the publisher at administration read. In an owner-authenticated shell, after applying the repository protections, run:

```sh
IOS_RELEASE_CONFIG=/path/to/app/.github/ios-release.json \
  python3 scripts/ci/capture_controls.py --publisher-app-id YOUR_APP_ID --output /tmp/controls.json
```

Copy the resulting object into the app configuration's `github_controls` field and review it through the required PR. Capture compares owner-visible REST bypass actors with complete GraphQL actor nodes and checks that the ruleset did not change during capture. The baseline binds the publisher App ID, ruleset IDs, actor-node IDs and server-controlled modification timestamps. It contains no credentials.

Promotion checks all public protections live. Bootstrap freezes `github_controls` from protected main before loading archived app configuration. If REST bypass details are hidden, verification requires exact baseline IDs and modification instants (including fractional seconds, independent of timezone formatting), plus complete GraphQL counts and identities.

GitHub can redact a private Integration node itself as `[null]` even for that App's administration-read installation token. The only accepted redacted shape is one node, count one, no next page, exactly one owner-recorded Integration with `always` bypass, unchanged server-controlled ruleset IDs/time, and live `current_user_can_bypass: always`. The recorded App must still equal the configured publisher. This combines the owner-verified identity with an unchanged ruleset and the current token's effective permission; it does not claim the hidden identity was read directly. Empty, truncated, inaccessible, conflicting or changed evidence fails closed. Any ruleset edit requires owner inspection and a reviewed baseline update, even if benign. Never refresh the baseline automatically in a privileged release job or grant the publisher administration write merely to read its own controls.
