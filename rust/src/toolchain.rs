use crate::{
    config::App,
    fsutil,
    process::{Executor, Step},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Instant};

#[derive(Debug, Clone)]
pub struct Resolved {
    pub developer: String,
    pub devices: BTreeMap<String, String>,
    pub evidence: Value,
}

pub fn resolve(
    app: &App,
    compatibility: bool,
    names: &[String],
    executor: &mut impl Executor,
) -> Result<Resolved> {
    let expected = app.xcode(compatibility)?;
    let developer = expected["path"]
        .as_str()
        .context("Missing Xcode path")?
        .to_owned();
    let diagnostics = app.root.join("build/toolchain").join(if compatibility {
        "compatibility.json"
    } else {
        "primary.json"
    });
    fsutil::confined(&app.root, &diagnostics)?;
    let mut attempts = vec![];
    fsutil::json(&diagnostics, &json!({"probes":attempts}))?;
    let mut inspect = |program: &str, args: &[&str]| {
        // First use of another Xcode can start CoreSimulator and populate xcrun caches.
        // Only the read-only simulator inventory may retry a timeout; never retry app tests here.
        let inventory = args == ["simctl", "list", "devices", "available", "--json"];
        let step = Step::new(
            program,
            args.iter().copied(),
            if inventory { 120 } else { 60 },
        )
        .developer(&developer);
        probe(app, &step, inventory, &diagnostics, &mut attempts, executor)
    };
    let version = inspect("xcodebuild", &["-version"])?;
    ensure!(
        version.lines().collect::<Vec<_>>()
            == [
                format!(
                    "Xcode {}",
                    expected["version"]
                        .as_str()
                        .context("Missing Xcode version")?
                ),
                format!(
                    "Build version {}",
                    expected["build"].as_str().context("Missing Xcode build")?
                )
            ],
        "Installed Xcode differs from the configured version/build"
    );
    let sdk = inspect("xcrun", &["--sdk", "iphoneos", "--show-sdk-version"])?;
    ensure!(
        Some(sdk.as_str()) == expected["sdk"].as_str(),
        "Installed iPhoneOS SDK differs from configuration"
    );
    let runtime = format!(
        "com.apple.CoreSimulator.SimRuntime.iOS-{}",
        expected["runtime"]
            .as_str()
            .context("Missing runtime")?
            .replace('.', "-")
    );
    let devices = if names.is_empty() {
        BTreeMap::new()
    } else {
        let inventory: Value = serde_json::from_str(&inspect(
            "xcrun",
            &["simctl", "list", "devices", "available", "--json"],
        )?)?;
        resolve_inventory(&inventory, &runtime, names)?
    };
    let swift = inspect("xcrun", &["swift", "--version"])?;
    let macos = inspect("sw_vers", &["-productVersion"])?;
    let architecture = inspect("uname", &["-m"])?;
    let mut evidence = expected.clone();
    evidence["sdk"] = json!(sdk);
    evidence["swift"] = json!(swift);
    evidence["macos"] = json!(macos);
    evidence["architecture"] = json!(architecture);
    evidence["runner_image"] = json!(std::env::var("ImageVersion").ok());
    evidence["runner_image_os"] = json!(std::env::var("ImageOS").ok());
    evidence["simulators"] = json!(devices);
    Ok(Resolved {
        developer,
        devices,
        evidence,
    })
}

pub fn resolve_inventory(
    inventory: &Value,
    runtime: &str,
    names: &[String],
) -> Result<BTreeMap<String, String>> {
    let candidates = inventory["devices"][runtime]
        .as_array()
        .context("Configured simulator runtime is unavailable")?;
    let mut result = BTreeMap::new();
    for name in names {
        let matches: Vec<_> = candidates
            .iter()
            .filter(|d| d["name"].as_str() == Some(name) && d["isAvailable"] == true)
            .collect();
        ensure!(
            matches.len() == 1,
            "Expected one configured simulator: {name}; found {}",
            matches.len()
        );
        let id = matches[0]["udid"]
            .as_str()
            .context("Missing simulator UUID")?;
        crate::simulator::validate_id(id)?;
        ensure!(
            !result
                .values()
                .any(|existing: &String| existing.eq_ignore_ascii_case(id)),
            "Configured simulator names resolve to the same UUID"
        );
        ensure!(
            result.insert(name.clone(), id.to_owned()).is_none(),
            "Duplicate requested simulator"
        );
    }
    Ok(result)
}

fn probe(
    app: &App,
    step: &Step,
    retry_timeout: bool,
    diagnostics: &Path,
    attempts: &mut Vec<Value>,
    executor: &mut impl Executor,
) -> Result<String> {
    let command = serde_json::to_value(step)?;
    for attempt in 1..=2 {
        let started = Instant::now();
        let result = executor.run(step, &app.root, None);
        let record = match &result {
            Ok(output) => {
                json!({"command":command,"attempt":attempt,"elapsed_seconds":started.elapsed().as_secs_f64(),"status":output.status,"stdout":output.stdout})
            }
            Err(error) => {
                json!({"command":command,"attempt":attempt,"elapsed_seconds":started.elapsed().as_secs_f64(),"error":error.to_string()})
            }
        };
        attempts.push(record);
        fsutil::json(diagnostics, &json!({"probes":attempts}))?;
        let output = result.with_context(|| {
            format!(
                "Toolchain query {command} failed; see {}",
                diagnostics.display()
            )
        })?;
        if output.status == 124 && retry_timeout && attempt == 1 {
            eprintln!(
                "Simulator inventory timed out after {} seconds; retrying once. Diagnostics: {}",
                step.timeout_seconds,
                diagnostics.display()
            );
            continue;
        }
        ensure!(
            output.status == 0,
            "Toolchain query {command} exited {}; see {}",
            output.status,
            diagnostics.display()
        );
        return Ok(output.stdout.trim().to_owned());
    }
    unreachable!("The second probe always returns")
}
