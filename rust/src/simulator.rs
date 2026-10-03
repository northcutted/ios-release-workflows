use crate::{
    fsutil,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, time::Instant};

pub fn validate_id(value: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(value).context("Exact simulator UUID is required")?;
    ensure!(
        parsed.hyphenated().to_string().eq_ignore_ascii_case(value),
        "Exact simulator UUID is required"
    );
    Ok(())
}

pub fn prepare(
    id: &str,
    developer: &str,
    root: &Path,
    output: &Path,
    executor: &mut impl Executor,
) -> Result<()> {
    validate_id(id)?;
    let started = Instant::now();
    let mut evidence = json!({"udid":id,"status":"failed","timeout_seconds":180});
    let operation = executor.run(
        &Step::new("xcrun", ["simctl", "bootstatus", id, "-b"], 180).developer(developer),
        root,
        None,
    );
    let result = match operation {
        Ok(result) => {
            evidence["boot_log"] = json!(result.stdout);
            evidence["exit_status"] = json!(result.status);
            if result.status == 0 {
                evidence["status"] = json!("ready");
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "Selected simulator readiness failed: exit {}",
                    result.status
                ))
            }
        }
        Err(error) => {
            evidence["error"] = json!(error.to_string());
            Err(error)
        }
    };
    evidence["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    fsutil::json(output, &evidence)?;
    result
}

pub fn reset(
    id: &str,
    selected: &str,
    developer: &str,
    root: &Path,
    executor: &mut impl Executor,
) -> Result<()> {
    validate_id(id)?;
    ensure!(
        id.eq_ignore_ascii_case(selected),
        "Reset must target the exact resolved simulator"
    );
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted"),
        "Automatic reset requires a disposable GitHub-hosted runner"
    );
    let inventory: Value = serde_json::from_str(&checked(
        executor,
        &Step::new(
            "xcrun",
            ["simctl", "list", "devices", "available", "--json"],
            30,
        )
        .developer(developer),
        root,
    )?)?;
    let devices = inventory["devices"]
        .as_object()
        .context("Missing simulator inventory")?;
    let matches: Vec<_> = devices
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter(|d| {
            d["udid"]
                .as_str()
                .is_some_and(|v| v.eq_ignore_ascii_case(id))
                && d["isAvailable"] == true
        })
        .collect();
    ensure!(
        matches.len() == 1,
        "Selected simulator is not uniquely available"
    );
    match matches[0]["state"].as_str() {
        Some("Booted") => {
            checked(
                executor,
                &Step::new("xcrun", ["simctl", "shutdown", id], 60).developer(developer),
                root,
            )?;
        }
        Some("Shutdown") => {}
        _ => anyhow::bail!("Unexpected simulator state; recovery refused"),
    }
    checked(
        executor,
        &Step::new("xcrun", ["simctl", "erase", id], 60).developer(developer),
        root,
    )?;
    Ok(())
}

pub fn inspect(
    bundle: &Path,
    developer: &str,
    root: &Path,
    output: &Path,
    executor: &mut impl Executor,
) -> Result<bool> {
    let mut assessment =
        json!({"recoverable":false,"reason":"Missing or unrecognized XCTest evidence"});
    let result = (|| -> Result<bool> {
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
                        bundle.to_str().context("Invalid result bundle path")?,
                        "--compact",
                    ],
                    30,
                )
                .developer(developer),
                root,
            )?;
            fsutil::atomic(
                &output
                    .parent()
                    .context("Missing assessment parent")?
                    .join(format!("{report}.json")),
                raw.as_bytes(),
            )?;
            reports.push(serde_json::from_str::<Value>(&raw)?);
        }
        Ok(crate::results::bootstrap_failure(&reports[0], &reports[1]))
    })();
    match result {
        Ok(recoverable) => {
            assessment["recoverable"] = json!(recoverable);
        }
        Err(error) => {
            assessment["error"] = json!(error.to_string());
        }
    }
    fsutil::json(output, &assessment)?;
    Ok(assessment["recoverable"] == true)
}
