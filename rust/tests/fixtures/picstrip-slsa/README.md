# Signed provenance fixture

These two unmodified public assets come from [PicStrip v1.7.0](https://github.com/northcutted/picstrip/releases/tag/v1.7.0):

- `release-build-manifest.json`
- `provenance.intoto.jsonl`

The signature was verified with SLSA verifier 2.7.1. The isolated builder is
`slsa-framework/slsa-github-generator` v2.1.0, source commit
`c042d27c17cb359c815dc0744f312c6edc00d2df`, workflow `.github/workflows/main.yml`,
run `35466614779`, attempt `1`. This is a legacy push preparation, used only to
exercise real cryptographic verification and parsing. It is not evidence that
the new native preparation has run or delivered an Apple build.

The ignored integration test verifies this signed manifest, rejects altered
manifest bytes, and rejects a different expected run. Linux CI installs the
reviewed verifier and runs that test explicitly. No Apple credentials are used.
