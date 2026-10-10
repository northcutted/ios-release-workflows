# Signing

[Documentation](README.md) · [Quickstart](native-quickstart.md) · [Releases](native-releases.md)

Set your Apple team and tracking/encryption declarations for every target before
signing. Certificate issuance and renewal are implemented but still await live
adoption proof; see [beta status](native-status.md).

## Connect Apple access

Your Apple Developer membership must be active. Create a team API key with access
to Certificates, Identifiers & Profiles, download its `.p8` file once, then save it
privately for local use:

```sh
ios-release auth login --key-id YOURKEYID --issuer-id YOURISSUERID --key-file ~/Downloads/AuthKey_YOURKEYID.p8
ios-release signing sync
ios-release signing status
```

`signing sync` prompts for a signing-vault password of at least 16 characters. Keep it in your password
manager. It registers bundle IDs, enables declared capabilities, reuses a matching
distribution certificate, creates app-specific App Store profiles, and configures
manual signing for the archive configuration. Debug and test signing stay separate.
Certificates and profiles renew within 30 days of expiry. The CLI never revokes
unrelated certificates.

Commit `.ios-release/signing.vault` with the app configuration and project changes.
The vault contains authenticated, encrypted signing assets; the password and Apple
API private key stay outside the repository. Preserve a password-manager copy of
the password and an encrypted repository backup.

## Import an existing identity

If the app already has a distribution identity, import it first:

```sh
export IOS_RELEASE_P12_PASSWORD='the existing identity password'
ios-release signing import --p12 /private/path/distribution.p12
ios-release signing sync
unset IOS_RELEASE_P12_PASSWORD
```

## Keychains, capabilities, and Apple setup

Signing installation uses an owned temporary keychain and removes only its own
profiles/keychain afterward. It does not replace the user's default keychain.
App groups, iCloud containers and Apple Pay merchant associations must already be
configured in Apple Developer; profile validation explains missing entitlements.

Create the app record once in [App Store Connect](https://appstoreconnect.apple.com/apps)
using the discovered main bundle ID. Apple does not expose app-record creation
through its public API. Complete the app's privacy, age rating, pricing, availability
and account agreements there. The CLI links back to outstanding Apple requirements
when a submission is incomplete.
