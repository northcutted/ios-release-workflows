use crate::{archive, config::App, fsutil, process::Executor, qa, signing};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub fn verify_qa(app: &App) -> Result<Value> {
    let inputs = crate::source::fingerprint(&app.root)?;
    let mut results = json!({});
    let checks = app.config["qa_checks"]
        .as_array()
        .map(|v| v.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["analyze", "localization", "test"]);
    ensure!(
        checks.contains(&"test"),
        "Release QA must include executed app tests"
    );
    for check in checks {
        let path = app.root.join("qa-results").join(check).join("result.json");
        let mut value: Value = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("Run qa {check} before sealing a release"))?,
        )?;
        ensure!(value["status"] == 0, "QA check {check} failed");
        ensure!(
            value["inputs_sha256"] == inputs,
            "QA check {check} is stale; rerun QA after changing app inputs"
        );
        if let Ok(source) = std::env::var("SOURCE_SHA").or_else(|_| std::env::var("GITHUB_SHA")) {
            ensure!(
                value["source_sha"] == source,
                "QA check {check} belongs to another source commit"
            );
        }
        if let Ok(run) = std::env::var("GITHUB_RUN_ID") {
            ensure!(
                value["run_id"] == run,
                "QA check {check} belongs to another workflow run"
            );
        }
        if check.starts_with("test") {
            let summary: Value =
                serde_json::from_slice(&fs::read(path.with_file_name("summary.json"))?)?;
            let tests: Value =
                serde_json::from_slice(&fs::read(path.with_file_name("tests.json"))?)?;
            let report = crate::results::junit(&summary, &tests)?;
            ensure!(
                report.passed && report.executed > 0,
                "App tests did not execute successfully"
            );
            value["summary"] = summary;
            value["tests"] = tests;
        }
        results[check] = value;
    }
    Ok(results)
}
pub fn seal(app: &App, version: &str, number: &str, directory: &Path) -> Result<Value> {
    app.require_archive()?;
    let qa = verify_qa(app)?;
    let archive: Value =
        serde_json::from_slice(&fs::read(app.root.join("build/rust-archive.json"))?)?;
    ensure!(
        archive["version"] == version && archive["build_number"] == number,
        "Archive identity differs from release"
    );
    ensure!(
        archive["inputs_sha256"] == crate::source::fingerprint(&app.root)?,
        "Archive is stale; rerun preparation after changing app inputs"
    );
    for value in qa.as_object().unwrap().values() {
        ensure!(
            value["source_sha"] == archive["source_sha"] && value["run_id"] == archive["run_id"],
            "QA and archive have different source/run identities"
        );
    }
    let ipa = app.root.join("build/application.ipa");
    ensure!(
        archive["ipa_sha256"].as_str() == Some(&fsutil::sha256(&ipa)?),
        "Archive IPA changed after validation"
    );
    fsutil::confined(&app.root, directory)?;
    ensure!(
        !directory.exists(),
        "A sealed release already exists; use its receipt instead of overwriting it"
    );
    fs::create_dir_all(directory.parent().context("Release needs a parent")?)?;
    let temporary = tempfile::tempdir_in(directory.parent().unwrap())?;
    let staged = temporary.path();
    fs::copy(&ipa, staged.join("application.ipa"))?;
    fsutil::json(&staged.join("app.json"), &app.config)?;
    fsutil::json(&staged.join("qa.json"), &qa)?;
    fsutil::json(&staged.join("archive.json"), &archive)?;
    let source = std::env::var("SOURCE_SHA")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .unwrap_or_else(|_| "local".into());
    let manifest = json!({"schema_version":2,"verified":true,"source_sha":source,"run_id":archive["run_id"],"platform_revision":env!("IOS_RELEASE_BUILD_REVISION"),"bundle_id":app.config["app_store"]["bundle_id"],"team_id":app.config["team_id"],"version":version,"build_number":number,"ipa_sha256":fsutil::sha256(&staged.join("application.ipa"))?,"config_sha256":fsutil::sha256(&staged.join("app.json"))?,"qa_sha256":fsutil::sha256(&staged.join("qa.json"))?,"archive_sha256":fsutil::sha256(&staged.join("archive.json"))?});
    fsutil::json(&staged.join("release.json"), &manifest)?;
    fs::rename(staged, directory)?;
    Ok(manifest)
}
pub fn prepare(
    app: &mut App,
    version: &str,
    number: &str,
    executor: &mut impl Executor,
    managed: bool,
) -> Result<Value> {
    let checks = app.config["qa_checks"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["analyze".into(), "localization".into(), "test".into()]);
    for check in checks {
        ensure!(
            qa::run(app, &check, executor)? == 0,
            "Release stopped because {check} failed"
        );
    }
    let _installed = if managed {
        Some(signing::Installed::install(app, executor)?)
    } else {
        None
    };
    archive::run(app, version, number, executor)?;
    let directory = app.root.join("build/native-release");
    let value = seal(app, version, number, &directory)?;
    println!(
        "Release prepared at {}. Next: ios-release store upload --release {}, then store wait and store stage.",
        directory.display(),
        directory.display()
    );
    Ok(value)
}
