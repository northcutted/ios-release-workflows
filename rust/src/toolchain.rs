use crate::{
    config::App,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
    let invoke = |program: &str, args: &[&str]| {
        Step::new(program, args.iter().copied(), 30).developer(&developer)
    };
    let version = checked(executor, &invoke("xcodebuild", &["-version"]), &app.root)?;
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
    let sdk = checked(
        executor,
        &invoke("xcrun", &["--sdk", "iphoneos", "--show-sdk-version"]),
        &app.root,
    )?;
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
        let inventory: Value = serde_json::from_str(&checked(
            executor,
            &invoke(
                "xcrun",
                &["simctl", "list", "devices", "available", "--json"],
            ),
            &app.root,
        )?)?;
        resolve_inventory(&inventory, &runtime, names)?
    };
    let swift = checked(
        executor,
        &invoke("xcrun", &["swift", "--version"]),
        &app.root,
    )?;
    let macos = checked(
        executor,
        &Step::new("sw_vers", ["-productVersion"], 30),
        &app.root,
    )?;
    let architecture = checked(executor, &Step::new("uname", ["-m"], 30), &app.root)?;
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
            result.insert(name.clone(), id.to_owned()).is_none(),
            "Duplicate requested simulator"
        );
    }
    Ok(result)
}
