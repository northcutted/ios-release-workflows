//! Isolated generator proof is verified independently of GitHub build attestations.
use crate::{
    config::App,
    fsutil,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Write, path::Path};

pub const BUILDER: &str = "https://github.com/slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@refs/tags/v2.1.0";
pub const PROOF: &str = "provenance.intoto.jsonl";
pub const BUNDLE: &str = "build-attestation.jsonl";

#[derive(Clone, Debug)]
pub struct Identity {
    pub repository: String,
    pub source_sha: String,
    pub workflow: String,
    pub run: u64,
    pub attempt: u64,
    pub event: String,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")?.is_match(&self.repository)
                && self.source_sha.len() == 40
                && self.source_sha.bytes().all(|c| c.is_ascii_hexdigit())
                && self.run > 0
                && self.attempt > 0
                && ["push", "workflow_dispatch"].contains(&self.event.as_str()),
            "Invalid preparation identity"
        );
        validate_workflow(&self.workflow)
    }
    pub fn current(app: &App) -> Result<Self> {
        ensure!(
            std::env::var("GITHUB_REF").as_deref() == Ok("refs/heads/main")
                && std::env::var("GITHUB_EVENT_NAME").as_deref() == Ok("workflow_dispatch"),
            "Preparation verification requires a dispatch from main"
        );
        let repository = crate::github::repository(app)?.to_owned();
        let reference = std::env::var("GITHUB_WORKFLOW_REF")?;
        ensure!(
            reference.ends_with("@refs/heads/main"),
            "Preparation workflow must come from main"
        );
        let identity = Self {
            workflow: workflow_path(&repository, &reference)?,
            repository,
            source_sha: std::env::var("GITHUB_SHA")?,
            run: std::env::var("GITHUB_RUN_ID")?.parse()?,
            attempt: std::env::var("GITHUB_RUN_ATTEMPT")?.parse()?,
            event: "workflow_dispatch".into(),
        };
        identity.validate()?;
        Ok(identity)
    }
}
pub fn validate_workflow(path: &str) -> Result<()> {
    ensure!(
        regex::Regex::new(r"^\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml$")?.is_match(path),
        "Invalid consumer workflow path"
    );
    Ok(())
}
pub fn workflow_path(repository: &str, reference: &str) -> Result<String> {
    let (path, reference) = reference
        .strip_prefix(&format!("{repository}/"))
        .context("Workflow belongs to another repository")?
        .split_once('@')
        .context("Workflow ref missing")?;
    ensure!(
        reference.starts_with("refs/")
            && reference.len() > 5
            && !reference.contains(['\n', '\r', '\0']),
        "Invalid workflow ref"
    );
    validate_workflow(path)?;
    Ok(path.to_owned())
}

pub fn subjects(
    app: &App,
    directory: &Path,
    package: &Path,
    output: Option<&Path>,
) -> Result<String> {
    crate::store::Release::load(app, directory)?;
    fsutil::confined(&app.root, package)?;
    ensure!(
        package.is_file() && !package.is_symlink(),
        "Release package missing or unsafe"
    );
    let files = hashes(directory, package)?;
    let text: String = files
        .iter()
        .map(|(name, digest)| format!("{digest}  {name}\n"))
        .collect();
    let encoded = STANDARD.encode(text);
    if let Some(path) = output {
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        writeln!(file, "subjects={encoded}")?;
    }
    Ok(encoded)
}
pub fn hashes(directory: &Path, package: &Path) -> Result<BTreeMap<String, String>> {
    Ok(BTreeMap::from([
        ("native-release.zip".into(), fsutil::sha256(package)?),
        (
            "release.json".into(),
            fsutil::sha256(&directory.join("release.json"))?,
        ),
        (
            "application.ipa".into(),
            fsutil::sha256(&directory.join("application.ipa"))?,
        ),
    ]))
}

/// Only call on a statement returned by the cryptographic verifier, never a decoded envelope.
pub fn validate_identity(statement: &Value, expected: &Identity) -> Result<()> {
    expected.validate()?;
    let predicate = &statement["predicate"];
    let source = &predicate["invocation"]["configSource"];
    let environment = &predicate["invocation"]["environment"];
    let run = expected.run.to_string();
    let attempt = expected.attempt.to_string();
    ensure!(
        statement["_type"] == "https://in-toto.io/Statement/v0.1"
            && statement["predicateType"] == "https://slsa.dev/provenance/v0.2"
            && predicate["builder"]["id"] == BUILDER
            && predicate["buildType"]
                == "https://github.com/slsa-framework/slsa-github-generator/generic@v1",
        "Expected the reviewed isolated SLSA generator"
    );
    ensure!(
        source["uri"]
            == format!(
                "git+https://github.com/{}@refs/heads/main",
                expected.repository
            )
            && source["digest"]["sha1"] == expected.source_sha
            && source["entryPoint"] == expected.workflow,
        "Isolated provenance source or workflow differs from selection"
    );
    ensure!(
        environment["github_run_id"].as_str() == Some(run.as_str())
            && environment["github_run_attempt"].as_str() == Some(attempt.as_str())
            && environment["github_event_name"] == expected.event
            && environment["github_ref"] == "refs/heads/main",
        "Isolated provenance belongs to another run, attempt or event"
    );
    Ok(())
}
pub fn validate_subjects(statement: &Value, expected: &BTreeMap<String, String>) -> Result<()> {
    let mut found = BTreeMap::new();
    for subject in statement["subject"]
        .as_array()
        .context("SLSA subjects missing")?
    {
        let name = subject["name"]
            .as_str()
            .context("SLSA subject name missing")?;
        let digest = subject["digest"]["sha256"]
            .as_str()
            .context("SLSA subject checksum missing")?;
        ensure!(
            found.insert(name.to_owned(), digest.to_owned()).is_none(),
            "Duplicate SLSA subject"
        );
    }
    ensure!(
        &found == expected,
        "SLSA subjects differ from the exact package, manifest or IPA"
    );
    Ok(())
}
pub fn verify(
    package: &Path,
    proof: &Path,
    expected: &Identity,
    root: &Path,
    executor: &mut impl Executor,
) -> Result<Value> {
    expected.validate()?;
    ensure!(
        proof.is_file() && !proof.is_symlink() && fs::metadata(proof)?.len() <= 2 * 1024 * 1024,
        "Isolated provenance is missing, unsafe or oversized"
    );
    let raw = checked(
        executor,
        &Step::new(
            "slsa-verifier",
            [
                "verify-artifact",
                package.to_str().context("Invalid package path")?,
                "--provenance-path",
                proof.to_str().context("Invalid provenance path")?,
                "--source-uri",
                &format!("github.com/{}", expected.repository),
                "--source-branch",
                "main",
                "--builder-id",
                BUILDER,
                "--print-provenance",
            ],
            180,
        ),
        root,
    )?;
    let statement: Value = serde_json::from_str(&raw)?;
    validate_identity(&statement, expected)?;
    Ok(statement)
}

pub fn validate_manifest(manifest: &Value, expected: &Identity) -> Result<()> {
    let run = expected.run.to_string();
    let attempt = expected.attempt.to_string();
    ensure!(
        manifest["source_sha"] == expected.source_sha
            && manifest["source_workflow"] == expected.workflow
            && manifest["run_id"].as_str() == Some(run.as_str())
            && manifest["run_attempt"].as_str() == Some(attempt.as_str())
            && manifest["repository"] == expected.repository
            && manifest["platform_revision"] == env!("IOS_RELEASE_BUILD_REVISION"),
        "Preparation manifest differs from authenticated source, workflow, run or platform"
    );
    Ok(())
}
