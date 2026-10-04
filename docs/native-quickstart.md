# Ship a native iPhone/iPad app

The native CLI manages distribution signing, app QA, archive/export, TestFlight,
store content, App Review and production releases. It runs as one Rust executable;
Ruby, Python, Node, Bundler and Fastlane are not runtime dependencies.

The native path is an opt-in beta. Existing apps can keep their current release
workflow while rehearsing it in a separate checkout. PicStrip's current workflow
and platform pin remain on the established release path until fresh signed parity
and native Apple delivery have been verified.

## Install

Install [GitHub CLI](https://cli.github.com) and authenticate with `gh auth login`.
Download a native CLI version from the platform's
[releases](https://github.com/northcutted/ios-release-workflows/releases).
The installer verifies the checksum and GitHub provenance before executing it:

```sh
bash /path/to/ios-release-workflows/scripts/install-native.sh 0.2.0-beta.1
~/.local/bin/ios-release doctor
```

Use the full binary path if another `ios-release` command already owns your PATH.
Before the first beta is published, build a reviewed checkout with
`cargo build --manifest-path rust/Cargo.toml --locked --release` and invoke
`rust/target/release/ios-release` explicitly.

Building and simulator tests require macOS, Xcode and an installed iOS runtime.
Signing uses OpenSSL and macOS Keychain tools. Linux binaries support Apple API
and release-management operations. SwiftLint is needed only when you enable lint.

## Connect your app

Open the app's shared scheme in Xcode and include a unit test target. From the app
repository, run:

```sh
ios-release init
ios-release qa all
```

Initialization discovers the app and extension bundle IDs, signing targets,
shared scheme, test targets, archive configuration, installed Xcode and simulator.
It creates `.ios-release.json`, starter store content and a workflow pinned to the
binary's source revision. GitHub repository information comes from the origin
remote; use `--repository owner/name` if the remote is not on GitHub.

Choose a scheme explicitly when the project has several:

```sh
ios-release init --workspace Example.xcworkspace --scheme Example
```

The CLI does not invent compliance answers. Set the Apple team and answer tracking
and encryption for your app before archiving:

```sh
ios-release init --team-id YOURTEAMID --tracking false --non-exempt-encryption false
```

Those example answers apply only to apps that do not track people and do not use
non-exempt encryption. Apps with different extension policies can set each target
separately in `.ios-release.json`. Include a valid `PrivacyInfo.xcprivacy` in each
app/extension target and declare encryption in its Info.plist.

## Set up Apple access and signing

Your Apple Developer membership must be active. Create a team API key with access
to Certificates, Identifiers & Profiles, download its `.p8` file once, then save it
privately for local use:

```sh
ios-release auth login --key-id YOURKEYID --issuer-id YOURISSUERID --key-file ~/Downloads/AuthKey_YOURKEYID.p8
ios-release signing sync
ios-release signing status
```

`signing sync` prompts for a unique signing-vault password. Keep it in your password
manager. It registers bundle IDs, enables declared capabilities, reuses a matching
distribution certificate, creates app-specific App Store profiles, and configures
manual signing for the archive configuration. Debug and test signing stay separate.
Certificates and profiles renew within 30 days of expiry. The CLI never revokes
unrelated certificates.

Commit `.ios-release/signing.vault` with the app configuration and project changes.
The vault contains authenticated, encrypted signing assets; the password and Apple
API private key stay outside the repository. Preserve a password-manager copy of
the password and an encrypted repository backup.

If the app already has a distribution identity, import it first:

```sh
export IOS_RELEASE_P12_PASSWORD='the existing identity password'
ios-release signing import --p12 /private/path/distribution.p12
ios-release signing sync
unset IOS_RELEASE_P12_PASSWORD
```

Signing installation uses an owned temporary keychain and removes only its own
profiles/keychain afterward. It does not replace the user's default keychain.
App groups, iCloud containers and Apple Pay merchant associations must already be
configured in Apple Developer; profile validation explains missing entitlements.

Create the app record once in [App Store Connect](https://appstoreconnect.apple.com/apps)
using the discovered main bundle ID. Apple does not expose app-record creation
through its public API. Complete the app's privacy, age rating, pricing, availability
and account agreements there. The CLI links back to outstanding Apple requirements
when a submission is incomplete.

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

Set `app_store.testflight_groups` to Apple beta-group IDs. External groups trigger
beta review; Apple still decides approval. Set `app_store.release_type` to `MANUAL`
(default), `AFTER_APPROVAL`, or `SCHEDULED`; scheduled releases also need
`earliest_release_date`. Set `phased_release: true` to enable phased updates.

For a local rehearsal or a release without Actions:

```sh
ios-release release prepare --version 1.2.3
ios-release store upload
ios-release store wait
ios-release store testflight
ios-release store stage
ios-release store metadata
ios-release store submit --confirm
ios-release status
```

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

The separate [Orbit Notes app](../examples/OrbitNotes/README.md) is an unsigned
onboarding/screenshot fixture. Its hosted rehearsal complements the immutable
PicStrip canaries; neither substitutes for a fresh signed archive and live Apple
delivery validation before changing an existing app's default release path.
