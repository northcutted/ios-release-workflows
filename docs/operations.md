# Release and recover

This guide covers the existing Python/Ruby compatibility platform. For the native
Rust CLI, start with the [native quickstart](native-quickstart.md) or [documentation index](README.md).

[Start here](../README.md) · [Setup](setup.md) · [Operations](operations.md) · [Architecture](architecture.md) · [Reference](reference.md) · [Maintenance](maintenance.md)

## Release and recovery

Preparation produces schema-v3 evidence binding source, platform revision, app/team, configuration digest, QA (including localization), IPA/SBOMs, and run identity. Promotion verifies native attestations and isolated SLSA provenance, checks main ancestry, then rechecks the IPA immediately before upload. Artifacts expire after 90 days; expired candidates are not guessed or rebuilt during deployment.

To repair promotion tooling while reusing an existing candidate, update the pinned platform and explicitly add the candidate's reviewed producer commit to `trusted_producer_revisions` in the protected consumer configuration. This optional list accepts at most 20 full commit SHAs; the current platform commit is always included. Bootstrap captures this policy from the protected checkout before loading artifact configuration. An artifact cannot authorize its own producer. Each build and promotion signature must match its own recorded, approved commit and the fixed platform workflow identity; source, app/team, QA, SLSA and asset checks remain required. Also retain an approved prior promotion commit when reusing its processed handoff. Review each addition and remove historical approvals when their artifacts are no longer needed. This mechanism repairs deployment without rebuilding an already verified IPA.

GitHub loads a release-event caller from the original app tag. When promotion runs from a newer reviewed main commit, the publisher also creates `vVERSION-deploy-FULL_COMMIT` after immutable publication. Consumers must handle the `create` event for this protected tag and resolve the original release with `ios-release deployment-ref verify`. The tag uses the same publisher-only `v*` protection and staging/production environment rules. Its suffix must equal the event commit, and that commit must be on protected main. Deployment still authenticates the original immutable release and exact Apple build. An old release caller may reject a newer promotion signer; the deployment-tag run uses the corrected pinned tools. Retry by dispatching deployment on that exact deployment tag. Neither application tags nor published assets are moved.

`TESTFLIGHT_CANARY_ENABLED=true` allows manual TestFlight rehearsal without publication. `RELEASE_DISTRIBUTION_ENABLED=true` additionally permits publication/staging/submission. `publish: false` remains the promotion default. The default upload adapter is Transporter until a consumer validates the Build Uploads adapter in a real canary. Set the adapter in the candidate configuration and promotion input consistently; no automatic fallback exists.

The Build Uploads adapter reserves an Apple upload/file, transfers bounded byte ranges, commits SHA256, and binds Apple's processed build through the upload relationship. Failed attempts retain operation receipts. Rerun failed jobs in the **same promotion run** to recover prior successful transfer receipts. Existing uploads are reused only with a matching SHA256 or that run's verified successful Transporter receipt. Every successful canary also emits a signed final handoff artifact. To publish it later, supply the original candidate ID/digest plus that exact processed artifact ID/digest; the platform authenticates both, reads back the recorded Apple build, and performs no transfer. An ambiguous legacy upload without proof is rejected. Conflicting immutable releases are never replaced.

Staging explicitly attaches the processed build and rejects replacement of an already selected different build unless an exact reviewed replacement is configured as described below. Production approval precedes an explicit release-policy update and readback. Submission preserves permitted metadata edits, resumes only matching review items, and records success only after Apple's submitted state is visible. Receipts are separately attested workflow artifacts with 90-day retention; they do not mutate immutable releases and omit review credentials/contact details.

Metadata-only updates require an exact commit reachable from protected main. The operation records each consumed text file's hash and uses the same production submission gate. The observer changes no Apple state. It reports meaningful differences against the prior digest-checked observation, always refreshes the newest release, and keeps older active or unknown states under observation. A manual refresh checks older completed releases too. Observation snapshots are a polling/display cache, never release authorization evidence; a cache failure triggers a full refresh. Apple review outcomes, agreements/account setup, and owner-supplied compliance facts remain explicit responsibilities.

## Replacement builds

For a version still in `PREPARE_FOR_SUBMISSION`, review an optional consumer configuration entry:

```json
"replacement_release": {
  "version": "1.7.0",
  "source_tag": "v1.7.0",
  "build_number": "77.1",
  "app_store_build_id": "EXACT-PREVIOUS-APPLE-BUILD-ID"
}
```

The source tag must be the highest reachable stable release. Preparation retains that marketing version and assigns the new candidate a unique `v1.7.0-build-N.ATTEMPT` tag, bound to the exact new build number, source and configuration in its signed manifests. The previous tag, release and artifacts stay immutable. The new build number must exceed the declared old build. Remove the entry by PR when normal semantic versioning should resume.

Promotion still needs the exact candidate ID/digest, main ancestry, complete QA, signatures, provenance and protected environments. It does not upload automatically. Staging may replace only the declared Apple build ID and number for the same version, while that version is still `PREPARE_FOR_SUBMISSION` and no active review submission exists. A different selection or state fails before the relationship write. The previous selection is recorded and the new selection is read back; retries do not reselect a build that is already attached. Production approval and submission remain separate.

This supports an intentional replacement of an unsubmitted draft. It cannot roll back an approved App Store version, cancel a review, move an immutable tag, or bypass a release approval. Old manifests and stable release tags remain verifiable.

## Retry the original operation

| Failure | Resume from |
| --- | --- |
| Transfer or processing interruption | Re-run failed jobs in the original promotion run; retain transfer receipts. |
| Publication or staging interruption | Original failed run; published releases are authenticated and reused. |
| Metadata worker failed after dispatch succeeded | Deployment run for the generated operation tag. |
| Queued run | Check runner capacity and concurrency before treating it as failure. |
| Expired or conflicting evidence | Restore independently verified retained evidence where supported, or prepare and accept a new candidate. |
| Changed repository controls | Owner inspection and reviewed baseline refresh; see [maintenance](maintenance.md#read-only-repository-control-verification). |

Repository concurrency serializes Apple mutations only within one owning repo. Pending jobs can be superseded; explicitly rerun an intended superseded operation. Production approval stays required for every submission path. Public Actions artifacts are not private storage; preserve needed evidence before its configured expiry.
