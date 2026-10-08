# App Store screenshots

[Documentation](README.md) · [CLI reference](native-commands.md)

Use app-owned XCTest scenarios to capture iPhone/iPad store assets. The
[Orbit Notes example](../examples/OrbitNotes/README.md) provides a small native helper.

## Capture

The scheme and target must be explicit; the app continues to own its Swift screenshot scenarios. Existing SnapshotHelper cache files are supported without loading Fastlane.

```sh
ios-release screenshots \
  --scheme OrbitNotes --test-target OrbitNotesUITests \
  --only-testing OrbitNotesUITests/OnboardingTests/testStoreScreenshot \
  --devices 'iPhone 18 Pro Max' --languages en-US,ar-SA --isolated-cache
```

Set `screens` to `["01-notes"]`, `locales` to `["en-US", "ar-SA"]`, and
`screenshot_devices` to your installed device names in `.ios-release.json` for
this example. Devices and locales must be members of the app configuration. `--photo` and `--video` can seed app-relative fixture files. Capture uses a unique job directory and compiled test products. Raw output defaults to `build/rust-screenshots/images`; custom output must also remain below `build`. Existing captures move into the new job's evidence directory. New images become visible only after all requested captures and configured PNG dimensions pass. App-owned source screenshots are never overwritten by this command.

Existing SnapshotHelper versions read the real host's `Library/Caches/tools.fastlane` directory; XCTest overwrites `SIMULATOR_HOST_HOME`, so that variable cannot redirect their cache. The default bridge therefore requires a disposable GitHub-hosted runner. It atomically claims an absent cache directory, refuses any existing cache, and moves its owned files into the job evidence on exit. It never touches a developer's existing screenshot cache.

Apps whose helper explicitly reads `IOS_RELEASE_SNAPSHOT_HOME` can opt into `--isolated-cache`, including for local capture. The helper should use that environment variable before falling back to `SIMULATOR_HOST_HOME`; the corresponding `Library/Caches/tools.fastlane` directory then lives inside the job's app-local home. The [Orbit Notes helper](../examples/OrbitNotes/UITests/IosReleaseSnapshot.swift) supports this isolated mode.

One recovery is allowed per capture job, only on disposable GitHub-hosted runners and only for the recorded pre-test XCTest bootstrap signature. Recovery resets the exact resolved simulator UUID and reuses the same compiled test run. Assertion failures, app launch failures, executed tests and unknown evidence never trigger recovery. First-attempt logs, xcresults, readiness and recovery records are retained under `build/rust-screenshots/<job>`.

Screenshot composition remains app-owned. Native capture outputs opaque RGB PNGs suitable for Apple asset delivery. The native store commands upload these without loading Fastlane.
