use crate::{config::App, fsutil, signing};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

fn paths(app: &App) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    paths.insert(
        app.config_path
            .strip_prefix(&app.root)
            .context("Native configuration must stay inside the app")?
            .to_string_lossy()
            .into_owned(),
    );
    paths.insert(
        signing::path(app)?
            .strip_prefix(&app.root)?
            .to_string_lossy()
            .into_owned(),
    );
    for target in app.config["targets"]
        .as_array()
        .context("Signing targets missing")?
    {
        let project = target["project"]
            .as_str()
            .or_else(|| app.config["project"].as_str())
            .context("Target project path missing")?;
        paths.insert(format!("{project}/project.pbxproj"));
    }
    Ok(paths)
}
pub fn export(app: &App, output: &Path) -> Result<()> {
    let source = std::env::var("SOURCE_SHA")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .context("Signing transfer needs an explicit source SHA")?;
    let mut files = vec![];
    for name in paths(app)? {
        let path = app.root.join(&name);
        fsutil::confined(&app.root, &path)?;
        files.push(json!({"path":name,"sha256":fsutil::sha256(&path)?,"content":STANDARD.encode(fs::read(path)?)}));
    }
    fsutil::confined(&app.root, output)?;
    fsutil::json(
        output,
        &json!({"schema_version":1,"source_sha":source,"team_id":app.config["team_id"],"bundle_id":app.config["app_store"]["bundle_id"],"files":files}),
    )
}
pub fn apply(app: &App, input: &Path) -> Result<()> {
    let value: Value = serde_json::from_slice(&fs::read(input)?)?;
    let source = std::env::var("SOURCE_SHA")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .context("Signing transfer needs an explicit source SHA")?;
    ensure!(
        value["schema_version"] == 1
            && value["source_sha"] == source
            && value["team_id"] == app.config["team_id"]
            && value["bundle_id"] == app.config["app_store"]["bundle_id"],
        "Signing transfer belongs to another source/app/team"
    );
    let allowed = paths(app)?;
    let mut found = BTreeSet::new();
    let mut files = vec![];
    for entry in value["files"].as_array().context("Signing files missing")? {
        let name = entry["path"]
            .as_str()
            .context("Signing file path missing")?;
        ensure!(
            allowed.contains(name) && found.insert(name.to_owned()),
            "Unexpected or duplicate signing transfer file"
        );
        let path = app.root.join(crate::config::relative_path(name)?);
        fsutil::confined(&app.root, &path)?;
        let bytes = STANDARD.decode(entry["content"].as_str().context("Signing file missing")?)?;
        ensure!(bytes.len() < 16 * 1024 * 1024, "Signing file too large");
        use sha2::Digest;
        ensure!(
            format!("{:x}", sha2::Sha256::digest(&bytes)) == entry["sha256"].as_str().unwrap_or(""),
            "Signing transfer checksum differs"
        );
        files.push((path, bytes));
    }
    ensure!(found == allowed, "Signing transfer is incomplete");
    for (path, bytes) in files {
        fsutil::atomic(&path, &bytes)?;
    }
    Ok(())
}
