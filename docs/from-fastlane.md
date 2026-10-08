# Adopt alongside Fastlane

[Documentation](README.md) · [Quickstart](native-quickstart.md) · [Beta status](native-status.md)

Choose `ios-release` when your app fits a native Xcode release flow and you want
one executable, declarative configuration, generated Actions, and explicit build
identity/approval checks. Rehearse it alongside the release path you already trust.

[Fastlane](https://docs.fastlane.tools/) supports a broad action/plugin ecosystem,
custom lanes, and multiple platforms. Keep it for integrations outside this CLI's
scope. The benefit here is a smaller prescribed integration surface; it is not a
proven execution-speed advantage or complete Fastlane feature parity.

## What transfers?

| Existing app asset | Native approach |
| --- | --- |
| Xcode project, shared schemes, tests | Discover with `init`; review the generated targets/configurations |
| Distribution identity | Export the app's existing identity as a password-protected P12, then `signing import` |
| Match storage | Keep it for current releases; the native path uses its own encrypted vault and does not read a Match repository directly |
| Store text/screenshots | Point configuration at existing app-owned content, then `store validate`; review supported fields/formats |
| Snapshot UI tests | Keep scenarios; the legacy cache bridge runs only on disposable hosted runners, or use the native isolated helper |
| Custom lanes/plugins/hooks | Review each separately; arbitrary Fastfiles and release hooks are outside native privileged jobs |
| App versions/build numbers | Preserve the current version and monotonic Apple build numbers; preparation checks the next build number |
| Existing release evidence | Preserve its original producer and CLI; it is not interchangeable with a native preparation |

Fastlane's [Match documentation](https://docs.fastlane.tools/actions/match/) covers
its existing shared signing workflow. Importing an identity does not require
revoking or replacing it.

## Rehearsal

1. Use a separate clean checkout or worktree and a separate native install prefix.
   Avoid concurrent builds writing into the same app's `build/` directory.
2. Run `ios-release init --no-workflows` and review `.ios-release.json`. If the app
   also has `.github/ios-release.json`, pass **`--config .ios-release.json`** on native
   commands: the existing compatibility config takes precedence by default.
3. Run `ios-release --config .ios-release.json qa all`, compare test counts/results,
   and capture every configured screenshot locale/device. Source-bound reports
   must match the tested checkout.
4. Import existing signing assets and prepare a signed native release in that
   checkout. Compare bundle/team/version/build identities, entitlements, profiles,
   privacy declarations, dSYMs, and independently verified IPA bytes/evidence.
5. Complete the [remaining live gates](native-status.md#remaining-adoption-gates)
   with an explicitly chosen app/build before switching the app's workflow pin.
   Keep production approval and a reviewed rollback path.

When ready for native Actions, initialize a fresh adoption checkout with workflow
generation enabled and review the result. `init` updates declarations on an
existing native configuration; it does not regenerate a missing workflow for it.
The generated native workflow explicitly selects `.ios-release.json`.

Keep the existing Fastlane/compatibility files and original workflow pin available
through adoption. Updating this shared repository does not update a pinned app.
For PicStrip, the native canary remains separate from its established release path.
