# Orbit Notes

This small iPhone/iPad app rehearses onboarding without using PicStrip's project,
signing assets, Apple account or active simulators. It includes a share extension,
unit tests, a shared scheme with an `AppStore` archive configuration, and UI tests
that capture English and Arabic screenshots with the native helper.

The checked-in Xcode project is ready to open. XcodeGen is only needed when
editing `project.yml`; it is not part of app or CLI onboarding.

From a copy of this folder on a Mac with Xcode and an iOS runtime installed:

```sh
ios-release init --scheme OrbitNotes --no-workflows
ios-release qa test
```

The fixture declares no tracking or non-exempt encryption. These declarations
describe this fixture and must not be copied as answers for a different app.
Use your own bundle IDs and Apple team before attempting a signed release.

The platform's adoption workflow runs on disposable hosted runners. It does
not install signing material or distribute this fixture to Apple.
