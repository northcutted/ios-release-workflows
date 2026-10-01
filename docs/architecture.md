# Architecture and trust

[Start here](../README.md) · [Setup](setup.md) · [Operations](operations.md) · [Architecture](architecture.md) · [Reference](reference.md) · [Maintenance](maintenance.md)

## Ownership

The consumer owns application source, identity, policy, metadata, screenshot scenarios, and its operating guide. This repository owns shared implementation, configuration validation, locked dependencies, command contracts, documentation generation and system-level recovery guidance. Consumers adopt a full reviewed commit; platform changes do not silently alter their next build.

Privileged jobs never load consumer Fastfiles, Gemfiles or arbitrary release hooks. Approved Xcode build phases necessarily execute during compilation. Screenshot jobs may run app-owned scenarios without release secrets, using the platform's locked Ruby dependencies.

## Build once and carry the identity

Preparation runs archive/signing, QA and store packaging in parallel. QA reports, source, platform revision, app/team, configuration digest, signing identities, inventories, subject checksums and provenance form the candidate handoff. Promotion authenticates an exact artifact ID and digest before Apple access. Successful processing binds the exact Apple build to that candidate. Publication verifies assets before making the release immutable; staging and submission authenticate it again.

## Native QA and Fastlane

`ios-release qa` runs SwiftLint, localization audits and native `xcodebuild` analysis/tests. It requires the configured Xcode build, SDK and unique simulator. Test execution defaults to one worker. It writes `.xcresult`, raw logs, Apple's summary/test-tree JSON, JUnit and the existing source-bound `qa-results/<check>/result.json` contract. JUnit case counts and outcomes must match Apple's summary. Empty, missing, unknown, inconsistent or failing results fail the job; a failed xcodebuild exit cannot be made green by a report.

Ruby is limited to signing (`match`), archive/export (`gym`), screenshot capture (`snapshot`), TestFlight transfer and store delivery/prechecks. Exact-build validation, approval policy, provenance, recovery and submission control remain platform logic. The app has no separate Ruby lockfile. Local archives and screenshots use the same platform dependencies as hosted runs.

## Security and validation

The target is SLSA Build L3 for the GitHub-produced IPA, not Apple's redistributed binary. The isolated generator protects provenance keys from app compilation; native attestations additionally bind evidence and receipts. Source control, environment approvals, producer pinning, artifact verification, immutable publication, and consumer-owned credentials are separate required controls. Passing provenance verification alone is not certification of the complete deployment.

Run `make setup`, `make check`, then `python3 bin/ios-release setup --apple` and `python3 bin/ios-release check --apple` for Apple contracts. The actionlint adapter validates `$/` targets before normalizing their spelling for actionlint 1.7.12, which predates that documented GitHub syntax. No source files are rewritten by linting.

Accessibility synchronization preserves matching declarations and checks their readback. Draft updates contain feature booleans only; `deviceFamily` is creation-only. Changed published declarations require an explicitly prepared replacement draft.

Staging reads back an already-matching encryption declaration without rewriting it; missing or changed declarations still require a successful configured update and readback.

Screenshot processing waits at most two minutes per attempt. Locked Fastlane retries only incomplete images, keeps complete images with checksums, and fails after five attempts; the overall staging job is limited to 30 minutes.

After a successful canary, pass its exact `final-RUN-ATTEMPT` artifact ID and SHA256 as `processed_artifact_id` and `processed_sha256`. Promotion reads back the recorded Apple build and publishes the original signed handoff byte for byte. It does not upload or re-sign it. Interrupted publication resumes the recorded draft only when its handoff marker and every existing asset digest match; release lookup includes drafts, and all readbacks use the numeric release ID. Existing immutable releases can create a corrected protected deployment ref without changing the release or app tag.

Before enabling distribution: run complete native iOS QA, a signed candidate rehearsal, a TestFlight canary and draft inspection. Compare five equivalent runs before enabling two XCTest workers; require reliability and at least 15% median improvement. Local fixture tests do not substitute for a customer-owned external-repository or App Store rehearsal.
