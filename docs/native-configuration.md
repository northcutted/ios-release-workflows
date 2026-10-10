# App configuration

[Documentation](README.md) · [Quickstart](native-quickstart.md) · [Troubleshooting](troubleshooting.md)

Run `ios-release init` from your app folder to generate `.ios-release.json`.
Review the discovered values rather than copying another app's identities or
Xcode versions. The native configuration uses `schema_version: 2`.

If your app also has the compatibility `.github/ios-release.json`, select the
native file explicitly:

```sh
ios-release --config .ios-release.json qa all
```

For a terminal session that uses only the native CLI, you can instead set
`IOS_RELEASE_CONFIG=.ios-release.json`. Each native Actions job selects this file
explicitly. Keep the compatibility configuration and its existing workflow pin
through migration.

## Review after initialization

| Fields | What to check |
| --- | --- |
| `project` or `workspace`, `scheme` | The app's shared scheme and app-relative project/workspace path |
| `test_targets`, `test_device` | Unit test targets in that scheme and a unique available simulator name |
| `xcode` | Installed developer path, version, build, SDK, and iOS runtime; hosted builds need matching tools |
| `configurations.test`, `configurations.archive` | The scheme's test/archive configurations, including a custom App Store configuration |
| `repository` | The owning GitHub repository as `owner/name`; needed for Actions, optional for unsigned local QA |
| `team_id`, `targets` | Your Apple team and every app/extension's bundle ID, profile, entitlements, tracking, and encryption declarations |
| `app_store.bundle_id` | The main app's bundle ID, matching its App Store Connect record |

`init --no-workflows` is useful for unsigned evaluation. When `.ios-release.json`
already exists, `init` updates team/tracking/encryption declarations; it does not
rediscover targets/toolchains or regenerate workflows. Edit the reviewed fields
for later project changes. For a first Actions rehearsal after local-only setup,
use a fresh checkout with workflow generation enabled, as described in
[migration](from-fastlane.md#rehearsal).

## Choose your QA and store content

Default native preparation runs analysis, localization, and unit tests. To add
lint or compatibility testing, edit the generated configuration's fields:

```json
{
  "qa_checks": ["analyze", "localization", "test", "lint", "test-compatibility"],
  "locales": ["en-US", "ar-SA"],
  "screens": ["01-notes"],
  "metadata_path": "store/metadata",
  "screenshots_path": "store/screenshots"
}
```

This is an example of fields to edit, not a complete app configuration. Use your
app's actual locales and scene names. QA checks must be unique and include `test`.
Lint also needs SwiftLint and an app-owned `.swiftlint.yml`. Compatibility testing
needs a complete `compatibility` Xcode/SDK/runtime declaration.

`screenshot_devices` must contain each requested device name. `locales` selects
store/capture locales; `localization_catalogs` and `localization_locales` select
app string catalogs/translations for the localization audit. Capture assets using
[app-owned screenshot tests](native-screenshots.md), then run
`ios-release store validate` before delivery.

Choose `app_store.release_type` (`MANUAL`, `AFTER_APPROVAL`, or `SCHEDULED`),
`app_store.phased_release`, and your copyright before preparation. A scheduled
release also needs `app_store.earliest_release_date`. Preparation freezes its
release policy; manage that preparation using its original reviewed CLI and pin.
Use [beta group setup](native-releases.md#store-content-and-ongoing-releases) to
save app-owned TestFlight group IDs without looking them up manually.

## What belongs in Git?

| Commit with your app | Keep private or generated |
| --- | --- |
| `.ios-release.json`, generated caller workflow, reviewed project changes | Apple `.p8` API key and local auth profile |
| `.ios-release/signing.vault` (encrypted) | Vault/P12 passwords and unencrypted signing identities |
| Store metadata/screenshots and app-owned screenshot tests | `build/`, `qa-results/`, project signing backups |

Native signing assets are encrypted in the vault; the password stays in your
password manager and the appropriate protected Actions environment. The local API
profile lives under `~/.config/ios-release/`. See [signing](native-signing.md) for
importing identities and preserving vault backups.
