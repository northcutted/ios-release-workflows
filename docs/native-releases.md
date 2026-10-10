# Releases and recovery

[Documentation](README.md) · [Quickstart](native-quickstart.md) · [Signing](native-signing.md)

Use one preparation for TestFlight, staging, and production. These native Apple
write operations have contract coverage and still await live adoption proof;
see [beta status](native-status.md).

## Enable GitHub Actions

With GitHub CLI authenticated and the same vault password available:

```sh
ios-release github setup
```

This sets native signing and store environments, their secrets and main-only
deployment rules. Your GitHub account is the default production reviewer; pass
`--reviewer username` to choose another owner. GitHub must support required
environment reviewers for your repository. The command verifies the resulting gate.

Commit the generated `.github/workflows/ios-native.yml` and open a PR. PRs run app QA
with read-only permissions and no signing/API secrets. They cannot prepare or ship.

In **Actions → iOS app → Run workflow**, use these operations:

| Operation | Result |
| --- | --- |
| Prepare | Reconciles signing, tests and builds in parallel, verifies the exported IPA and symbols, and seals an attested release |
| TestFlight | Uploads that exact preparation, waits for processing and assigns configured beta groups |
| Stage | Selects the same build and applies store metadata/screenshots without submitting it |
| Submit | Requests App Review after the production reviewer approves |
| Publish | Releases an approved version awaiting manual release, after production approval |
| Metadata | Applies updated store content to the staged version |
| Phase pause/resume/complete | Manages the configured phased release after production approval |
| Status | Reads current app/version/build state |

Prepare shows its run ID. Use that ID for delivery and later updates. The CLI
authenticates the successful main run, pinned producer, source ancestry, provenance
and exact IPA before Apple credentials become available, then verifies them again
inside the selected protected job. Recovery receipts are separately attested and
restored automatically, including receipts from interrupted operations.

Each preparation attempt retains separate artifacts. A failed-job rerun reuses
successful upstream artifacts by their exact IDs; selection verifies the successful
attempt's signed run identity. Once a preparation has been delivered, keep that
run unchanged and dispatch a new Prepare for the next build.

## Preserve and recover a release

Native release artifacts and recovery receipts are retained for 90 days. Download
and preserve a release directory for longer-term local management before expiry.
Do not delete successful preparation runs while their releases are being managed.
After upgrading the platform pin, manage earlier releases with their original
reviewed CLI version until the producer migration is explicitly validated.

## Store content and ongoing releases

Fill the locale folders in `store/metadata`: name, description, keywords, support
URL, privacy URL and release notes. Put opaque RGB PNGs in
`store/screenshots/<locale>`. Use app-owned UI-test scenarios and the
[native screenshot helper](../examples/OrbitNotes/UITests/IosReleaseSnapshot.swift)
to capture new assets. Screenshot publishing preserves existing assets; remove
superseded screenshots in App Store Connect if a device class reaches Apple's limit.

Run `ios-release store validate` to check every locale, URL and screenshot locally
before delivery. Starter descriptions and missing required fields stop delivery.
Optional `store/review.json` manages review contact details using Apple's fields:
`contactFirstName`, `contactLastName`, `contactPhone`, `contactEmail`,
`demoAccountRequired`, `demoAccountName` and `notes`. Supply a required demo password
privately as `IOS_RELEASE_DEMO_PASSWORD`; `github setup` stores it in the two store
environments when present. Otherwise, complete review contact details in App Store
Connect once.

Create and configure a beta group with `ios-release store beta-groups --create Team`
(add `--external` for external testers). The command reuses a matching app-owned
group and saves its ID, so you do not need to find resource IDs yourself. Commit
the updated configuration and manage testers in its printed App Store Connect link.
External groups trigger beta review; Apple still decides approval. Set
`app_store.copyright` to your copyright notice and `app_store.release_type` to `MANUAL`
(default), `AFTER_APPROVAL`, or `SCHEDULED`; scheduled releases also need
`earliest_release_date`. Set `phased_release: true` to enable phased updates.

## Use the local CLI

For a local rehearsal or a release without Actions, validate content before
preparation and run build selection and content delivery separately:

```sh
ios-release store validate
ios-release release prepare --version 1.2.3
ios-release store upload
ios-release store wait
ios-release store testflight
ios-release store stage
ios-release store metadata
ios-release store submit --confirm
ios-release status
```

`store stage` selects the exact processed build and applies its prepared release
policy. `store metadata` delivers content; the Actions **stage** operation runs
both automatically.

Preparation checks all configured QA, rejects stale source/test evidence, imports
signing assets temporarily and validates every exported app/extension. It selects
the next Apple build number unless you pass `--build-number`. Local preparation
does not mint GitHub provenance; local release operations are explicit owner actions.
Actions append the preparation run ID and attempt to the next major build number,
so independently prepared binaries have distinct build numbers before uploading.

To download an authenticated Actions preparation and resume it locally:

```sh
ios-release github fetch --run PREPARATION_RUN_ID
ios-release store status
ios-release store publish --confirm
```

Repeated upload, staging and review commands reconcile Apple state before mutation.
An ambiguous response stops and saves intent; the next command uses that receipt.
The CLI refuses mismatched apps, teams, versions, IPA bytes, foreign selected builds
and unrelated active review submissions. It prints IDs and public state, never API
tokens, upload capability URLs or private signing keys.
