use anyhow::Result;
use ios_release_native::{
    process::{Arg, Executor, Native, Output, Step},
    provenance::{self, Identity},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn identity() -> Identity {
    Identity {
        repository: "owner/app".into(),
        source_sha: "a".repeat(40),
        workflow: ".github/workflows/ios-release.yml".into(),
        run: 42,
        attempt: 2,
        event: "workflow_dispatch".into(),
    }
}
fn statement() -> Value {
    json!({"_type":"https://in-toto.io/Statement/v0.1","predicateType":"https://slsa.dev/provenance/v0.2",
        "subject":[{"name":"native-release.zip","digest":{"sha256":"b".repeat(64)}}],
        "predicate":{"builder":{"id":provenance::BUILDER},"buildType":"https://github.com/slsa-framework/slsa-github-generator/generic@v1",
            "invocation":{"configSource":{"uri":"git+https://github.com/owner/app@refs/heads/main","digest":{"sha1":"a".repeat(40)},"entryPoint":".github/workflows/ios-release.yml"},
                "environment":{"github_run_id":"42","github_run_attempt":"2","github_event_name":"workflow_dispatch","github_ref":"refs/heads/main"}}}})
}

#[test]
fn isolated_proof_requires_the_exact_generator_source_workflow_and_invocation() {
    let original = statement();
    provenance::validate_identity(&original, &identity()).unwrap();
    for (pointer, value) in [
        ("/_type", "https://in-toto.io/Statement/v1"),
        ("/predicateType", "https://slsa.dev/provenance/v1"),
        (
            "/predicate/builder/id",
            "https://example.invalid/not-isolated",
        ),
        ("/predicate/buildType", "unknown"),
        (
            "/predicate/invocation/configSource/uri",
            "git+https://github.com/owner/other@refs/heads/main",
        ),
        (
            "/predicate/invocation/configSource/digest/sha1",
            &"c".repeat(40),
        ),
        (
            "/predicate/invocation/configSource/entryPoint",
            ".github/workflows/other.yml",
        ),
        ("/predicate/invocation/environment/github_run_id", "43"),
        ("/predicate/invocation/environment/github_run_attempt", "1"),
        (
            "/predicate/invocation/environment/github_event_name",
            "pull_request",
        ),
        (
            "/predicate/invocation/environment/github_ref",
            "refs/heads/topic",
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = json!(value);
        assert!(
            provenance::validate_identity(&changed, &identity()).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn all_subjects_must_match_once_without_missing_or_additional_assets() {
    let expected = BTreeMap::from([
        ("native-release.zip".into(), "b".repeat(64)),
        ("application.ipa".into(), "c".repeat(64)),
        ("release.json".into(), "d".repeat(64)),
    ]);
    let mut original = statement();
    original["subject"] = json!(
        expected
            .iter()
            .map(|(name, hash)| json!({"name":name,"digest":{"sha256":hash}}))
            .collect::<Vec<_>>()
    );
    provenance::validate_subjects(&original, &expected).unwrap();
    for case in ["changed", "missing", "duplicate", "extra"] {
        let mut changed = original.clone();
        let subjects = changed["subject"].as_array_mut().unwrap();
        match case {
            "changed" => subjects[0]["digest"]["sha256"] = json!("e".repeat(64)),
            "missing" => {
                subjects.pop();
            }
            "duplicate" => subjects.push(subjects[0].clone()),
            _ => subjects.push(json!({"name":"other.ipa","digest":{"sha256":"f".repeat(64)}})),
        }
        assert!(
            provenance::validate_subjects(&changed, &expected).is_err(),
            "{case}"
        );
    }
}

struct Verifier {
    status: i32,
    stdout: String,
    calls: usize,
}
impl Executor for Verifier {
    fn run(&mut self, step: &Step, _: &Path, _: Option<&Path>) -> Result<Output> {
        self.calls += 1;
        assert_eq!(step.program, "slsa-verifier");
        let args: Vec<_> = step
            .args
            .iter()
            .map(|arg| match arg {
                Arg::Text(s) => s.as_str(),
                _ => panic!(),
            })
            .collect();
        assert!(
            args.windows(2)
                .any(|v| v == ["--builder-id", provenance::BUILDER])
        );
        assert!(args.windows(2).any(|v| v == ["--source-branch", "main"]));
        Ok(Output {
            status: self.status,
            stdout: self.stdout.clone(),
        })
    }
}

#[test]
fn only_successful_cryptographic_verifier_output_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("native-release.zip");
    let proof = root.path().join("provenance.intoto.jsonl");
    fs::write(&package, b"fixture").unwrap();
    fs::write(&proof, b"opaque signed fixture").unwrap();
    let mut verifier = Verifier {
        status: 1,
        stdout: statement().to_string(),
        calls: 0,
    };
    assert!(provenance::verify(&package, &proof, &identity(), root.path(), &mut verifier).is_err());
    verifier.status = 0;
    provenance::verify(&package, &proof, &identity(), root.path(), &mut verifier).unwrap();
    verifier.stdout = "{}".into();
    assert!(provenance::verify(&package, &proof, &identity(), root.path(), &mut verifier).is_err());
    fs::remove_file(&proof).unwrap();
    let before = verifier.calls;
    assert!(provenance::verify(&package, &proof, &identity(), root.path(), &mut verifier).is_err());
    assert_eq!(before, verifier.calls);
}

#[test]
fn workflow_and_manifest_identity_cannot_cross_repositories_runs_or_producers() {
    assert_eq!(
        provenance::workflow_path(
            "owner/app",
            "owner/app/.github/workflows/ios-release.yml@refs/heads/main"
        )
        .unwrap(),
        identity().workflow
    );
    for reference in [
        "other/app/.github/workflows/ios-release.yml@refs/heads/main",
        "owner/app/.github/workflows/../bad.yml@refs/heads/main",
        "owner/app/.github/workflows/ios-release.yml",
    ] {
        assert!(provenance::workflow_path("owner/app", reference).is_err());
    }
    let original = json!({"source_sha":"a".repeat(40),"source_workflow":identity().workflow,"run_id":"42","run_attempt":"2","repository":"owner/app","platform_revision":env!("IOS_RELEASE_BUILD_REVISION")});
    provenance::validate_manifest(&original, &identity()).unwrap();
    for key in [
        "source_sha",
        "source_workflow",
        "run_id",
        "run_attempt",
        "repository",
        "platform_revision",
    ] {
        let mut changed = original.clone();
        changed[key] = json!("wrong");
        assert!(
            provenance::validate_manifest(&changed, &identity()).is_err(),
            "{key}"
        );
    }
}

#[test]
#[ignore = "requires installed reviewed SLSA verifier and public trust roots"]
fn verifies_real_picstrip_provenance_and_rejects_tampered_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/picstrip-slsa");
    let package = root.join("release-build-manifest.json");
    let proof = root.join("provenance.intoto.jsonl");
    let expected = Identity {
        repository: "northcutted/picstrip".into(),
        source_sha: "c042d27c17cb359c815dc0744f312c6edc00d2df".into(),
        workflow: ".github/workflows/main.yml".into(),
        run: 35466614779,
        attempt: 1,
        event: "push".into(),
    };
    let verified = provenance::verify(&package, &proof, &expected, &root, &mut Native).unwrap();
    assert_eq!(verified["subject"].as_array().unwrap().len(), 3);
    let changed = tempfile::tempdir().unwrap();
    let changed_package = changed.path().join("release-build-manifest.json");
    fs::write(&changed_package, b"modified build manifest").unwrap();
    assert!(provenance::verify(&changed_package, &proof, &expected, &root, &mut Native).is_err());
    let mut wrong_run = expected;
    wrong_run.run += 1;
    assert!(provenance::verify(&package, &proof, &wrong_run, &root, &mut Native).is_err());
}
