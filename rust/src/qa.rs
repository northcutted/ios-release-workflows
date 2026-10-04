use crate::{
    config::App,
    fsutil,
    process::{Executor, Step, checked},
    results, simulator, toolchain,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub fn command(
    app: &App,
    name: &str,
    destination: &str,
    bundle: &Path,
    workers: &str,
) -> Result<Step> {
    ensure!(
        workers == "1" || workers == "2",
        "TEST_WORKERS must be 1 or 2"
    );
    let (kind, project) = app.project()?;
    let mut args = vec![
        format!("-{kind}"),
        project.into(),
        "-scheme".into(),
        app.text("scheme")?.into(),
        "-configuration".into(),
        app.configuration("test").into(),
        "-destination".into(),
        destination.into(),
        "CODE_SIGNING_ALLOWED=NO".into(),
    ];
    if name == "analyze" {
        args.extend(["-sdk", "iphonesimulator", "analyze"].map(String::from));
    } else {
        ensure!(
            !app.names("test_targets")?.is_empty(),
            "Add a unit test target to the shared scheme, then rerun init; release QA must execute app tests"
        );
        args.extend(
            [
                "-parallel-testing-enabled",
                if workers == "2" { "YES" } else { "NO" },
                "-parallel-testing-worker-count",
                workers,
                "-maximum-concurrent-test-simulator-destinations",
                workers,
                "-collect-test-diagnostics",
                "on-failure",
                "-resultBundlePath",
            ]
            .map(String::from),
        );
        args.push(bundle.to_string_lossy().into_owned());
        args.extend(
            app.names("test_targets")?
                .into_iter()
                .map(|name| format!("-only-testing:{name}")),
        );
        args.extend(["clean", "test"].map(String::from));
    }
    Ok(Step::new("xcodebuild", args, 2400).developer(
        app.xcode(name == "test-compatibility")?["path"]
            .as_str()
            .context("Missing Xcode path")?,
    ))
}

pub fn run(app: &App, name: &str, executor: &mut impl Executor) -> Result<i32> {
    let inputs_sha256 = crate::source::fingerprint(&app.root)?;
    ensure!(
        [
            "lint",
            "localization",
            "analyze",
            "test",
            "test-compatibility"
        ]
        .contains(&name),
        "Unknown QA check"
    );
    let directory = app.root.join("qa-results").join(name);
    fsutil::confined(&app.root, &directory)?;
    fsutil::confined(&app.root, &app.root.join("build"))?;
    fsutil::confined(&app.root, &app.root.join("build/test_output"))?;
    fs::create_dir_all(&directory)?;
    remove(&directory.join("report.junit"))?;
    if name.starts_with("test") {
        remove(&app.root.join("build/test_output/report.junit"))?;
    }
    let mut status = 1;
    let outcome = (|| -> Result<()> {
        if name == "localization" {
            let errors = crate::localization::audit(app)?;
            fsutil::atomic(&directory.join("output.log"), errors.join("\n").as_bytes())?;
            ensure!(
                errors.is_empty(),
                "{} string catalog problems; see retained output.log",
                errors.len()
            );
            status = 0;
            return Ok(());
        }
        if name == "lint" {
            status = executor
                .run(
                    &Step::new(
                        "swiftlint",
                        [
                            "lint",
                            "--strict",
                            "--no-cache",
                            "--config",
                            ".swiftlint.yml",
                        ],
                        300,
                    ),
                    &app.root,
                    Some(&directory.join("output.log")),
                )?
                .status;
            return Ok(());
        }
        let selected = toolchain::resolve(
            app,
            name == "test-compatibility",
            &[app.text("test_device")?.to_owned()],
            executor,
        )?;
        fsutil::json(&app.root.join("build/build-env.json"), &selected.evidence)?;
        let id = selected
            .devices
            .get(app.text("test_device")?)
            .context("Missing resolved test device")?;
        if name.starts_with("test") {
            simulator::prepare(
                id,
                &selected.developer,
                &app.root,
                &directory.join("simulator-readiness.json"),
                executor,
            )?;
        }
        let bundle = app
            .root
            .join("build/test_output")
            .join(format!("{name}-{}.xcresult", uuid::Uuid::new_v4()));
        fs::create_dir_all(bundle.parent().context("Missing result parent")?)?;
        let workers = std::env::var("TEST_WORKERS").unwrap_or_else(|_| "1".into());
        status = executor
            .run(
                &command(
                    app,
                    name,
                    &format!("platform=iOS Simulator,id={id}"),
                    &bundle,
                    &workers,
                )?,
                &app.root,
                Some(&directory.join("output.log")),
            )?
            .status;
        if name.starts_with("test") {
            let mut reports = Vec::new();
            for report in ["summary", "tests"] {
                let raw = checked(
                    executor,
                    &Step::new(
                        "xcrun",
                        [
                            "xcresulttool",
                            "get",
                            "test-results",
                            report,
                            "--path",
                            bundle.to_str().context("Invalid result path")?,
                            "--compact",
                        ],
                        30,
                    )
                    .developer(&selected.developer),
                    &app.root,
                )?;
                fsutil::atomic(&directory.join(format!("{report}.json")), raw.as_bytes())?;
                reports.push(serde_json::from_str::<Value>(&raw)?);
            }
            let result = results::junit(&reports[0], &reports[1])?;
            fsutil::atomic(&directory.join("report.junit"), result.xml.as_bytes())?;
            fsutil::atomic(
                &app.root.join("build/test_output/report.junit"),
                result.xml.as_bytes(),
            )?;
            if !result.passed && status == 0 {
                status = 1;
            }
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        if status == 0 {
            status = 1;
        }
        eprintln!("QA evidence failed: {error:#}");
    }
    fsutil::json(
        &directory.join("result.json"),
        &json!({"check":name,"status":status,"inputs_sha256":inputs_sha256,"source_sha":std::env::var("SOURCE_SHA").or_else(|_| std::env::var("GITHUB_SHA")).ok(),"run_id":std::env::var("GITHUB_RUN_ID").ok()}),
    )?;
    Ok(status)
}

fn remove(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
