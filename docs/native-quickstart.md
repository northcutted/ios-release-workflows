# Ship your first native app

[Documentation](README.md) · [Install](install.md) · [Beta status](native-status.md)

This guide takes a native Xcode app from local checks to a GitHub Actions release.
The native path is an opt-in beta; review its [validation status](native-status.md)
before replacing an existing production workflow.

## 1. Install and connect your app

[Install the verified binary](install.md). On a Mac with Xcode and an iOS runtime,
open your app's shared scheme and include a unit test target. From the app folder:

```sh
ios-release init
ios-release qa all
```

`init` discovers the project/workspace, shared scheme, test targets, app/extension
bundle IDs, archive configuration, Xcode, and simulator. It creates:

| File or folder | Yours to review |
| --- | --- |
| `.ios-release.json` | App settings, signing targets, QA, tracking/encryption declarations, store policy |
| `store/metadata/en-US/` | Starter store text and URLs; replace placeholders before delivery |
| `.github/workflows/ios-native.yml` | Workflow pinned to this binary's source and package |
| `.gitignore` additions | Build output, project backups, private signing file types |

For an app in a subdirectory, the workflow goes in the Git repository root and
selects that app folder. Repository identity comes from `origin`; use
`--repository owner/name` if necessary. For several projects or schemes, select one:

```sh
ios-release init --workspace Example.xcworkspace --scheme Example
```

In a terminal, initialization asks for your team, tracking, and encryption
declarations. You may defer answers for unsigned QA. Before signing, provide your
Apple team, answer those declarations for every target, include each target's
`PrivacyInfo.xcprivacy`, and declare encryption in its Info.plist. The CLI checks
your declarations; you determine the correct answers for your app.

Commit and review the generated configuration and workflow. The configured hosted
runner must contain the exact Xcode version/build, SDK, and simulator runtime.
Use `init --runner LABEL` to select a supported runner during initialization.

Want to try an app before connecting Apple? Use the
[Orbit Notes example](../examples/OrbitNotes/README.md).

## 2. Connect Apple and signing

Have an active Apple Developer membership and a team API key with access to
Certificates, Identifiers & Profiles. Download its `.p8` file once and store it
privately:

```sh
ios-release auth login --key-id YOURKEYID --issuer-id YOURISSUERID --key-file ~/Downloads/AuthKey_YOURKEYID.p8
ios-release signing sync
ios-release signing status
```

`signing sync` prompts for a vault password. Save it in your password manager.
The CLI manages distribution signing in `.ios-release/signing.vault` and configures
manual signing for the archive configuration. Commit the encrypted vault and
project/configuration changes; keep the password and API private key outside Git.

Already have a distribution identity? [Import it first](native-signing.md#import-an-existing-identity).
The [signing guide](native-signing.md) covers renewal, extensions, and capabilities.

Create your app record once in
[App Store Connect](https://appstoreconnect.apple.com/apps) using the main bundle
ID. Complete account agreements, privacy, age rating, pricing, and availability
there. These one-time Apple steps are still required.

## 3. Enable Actions and prepare

With GitHub CLI authenticated and your vault password available:

```sh
ios-release github setup
```

This configures native signing/store environments, secrets, main-only deployment
rules, and a production reviewer. Your GitHub account is the default reviewer;
`--reviewer username` selects another owner. The command verifies the gate. Your
GitHub plan must support attestations and required environment reviewers for this
repository; [private repository requirements](native-status.md#github-actions) apply.

Open a PR with the configuration, encrypted vault, and generated workflow. PRs run
QA without signing or Apple API secrets. After merging to protected main, open
**Actions → iOS app → Run workflow**, choose **prepare**, and enter your app version.

Preparation reconciles signing, builds and tests in parallel, independently
verifies the IPA and symbols, and seals an attested release. Save the successful
preparation's run ID from its summary.

## 4. Deliver and keep managing your app

Fill your store metadata and screenshots, then run:

```sh
ios-release store validate
```

In the same Actions workflow, supply the preparation run ID and choose:

1. **testflight** to upload the exact IPA, wait for Apple processing, and assign beta groups.
2. **stage** to select that build and apply store content.
3. **submit** to request App Review after production approval.
4. **publish** when an approved version is awaiting manual release.

Use **status** to read current Apple state. Later updates reuse the same workflow;
each new build starts with a new **prepare** run. Metadata and phased-release
operations select an existing preparation.

[Releases and recovery](native-releases.md) covers all operations, local delivery,
store content, interrupted requests, and preserving releases beyond 90 days.
[Screenshots](native-screenshots.md) covers native UI-test capture.
