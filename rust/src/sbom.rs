//! Source cataloguing and exact-IPA inventory. Neither scan executes application code.
use crate::{
    config::App,
    fsutil,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

pub const SYFT_VERSION: &str = "1.44.0";
const FILES: [&str; 3] = ["source.spdx.json", "ipa.spdx.json", "sbom.json"];
const POLICY: &str = "Apple SDK frameworks are platform dependencies; npm/Ruby/Python packages are build tools, not shipped app dependencies.";

fn read(path: &Path) -> Result<Value> {
    ensure!(
        path.is_file() && !path.is_symlink(),
        "Missing regular SBOM input: {}",
        path.display()
    );
    ensure!(
        fs::metadata(path)?.len() <= 32 * 1024 * 1024,
        "SBOM input exceeds 32 MiB"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn spdx(document: &Value) -> Result<()> {
    ensure!(
        document["spdxVersion"] == "SPDX-2.3"
            && document["SPDXID"] == "SPDXRef-DOCUMENT"
            && document["dataLicense"] == "CC0-1.0"
            && document["documentNamespace"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && document["creationInfo"]["creators"]
                .as_array()
                .is_some_and(|v| !v.is_empty()),
        "Expected a complete Syft SPDX 2.3 document"
    );
    let mut ids = BTreeSet::new();
    for package in document["packages"]
        .as_array()
        .context("SPDX packages missing")?
    {
        let id = package["SPDXID"]
            .as_str()
            .context("SPDX package ID missing")?;
        ensure!(
            id.starts_with("SPDXRef-") && ids.insert(id),
            "Duplicate or invalid SPDX package ID"
        );
    }
    Ok(())
}

/// Validate every ZIP entry before extracting it into a fresh private directory.
pub fn extract(ipa: &Path, destination: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(fs::File::open(ipa)?)?;
    ensure!(archive.len() <= 100000, "IPA has too many members");
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name().trim_end_matches('/');
        crate::config::relative_path(name)?;
        ensure!(
            !name.contains('\\')
                && name.split('/').all(|p| !p.is_empty() && p != ".")
                && file.enclosed_name().is_some()
                && !file.is_symlink()
                && names.insert(name.to_owned()),
            "Unsafe or duplicate IPA member"
        );
        if let Some(mode) = file.unix_mode() {
            ensure!(
                matches!(mode & 0o170000, 0 | 0o040000 | 0o100000),
                "Special IPA member"
            );
        }
        total = total
            .checked_add(file.size())
            .context("IPA size overflow")?;
        ensure!(
            file.size() <= 4 * 1024 * 1024 * 1024 && total <= 8 * 1024 * 1024 * 1024,
            "IPA exceeds extraction bounds"
        );
    }
    archive.extract(destination)?;
    Ok(())
}

fn plist_text<'a>(info: &'a plist::Value, key: &str) -> Result<&'a str> {
    info.as_dictionary()
        .and_then(|v| v.get(key))
        .and_then(plist::Value::as_string)
        .with_context(|| format!("IPA bundle field {key} missing"))
}

pub fn inventory(app: &App, root: &Path, archive: &Value) -> Result<Value> {
    crate::release::validate_inventory(app, archive)?;
    let verified = archive["applications"].as_array().unwrap();
    let mut applications = vec![];
    let mut embedded = vec![];
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.to_owned()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            ensure!(
                kind.is_file() || kind.is_dir(),
                "IPA contains a nonregular member"
            );
            if kind.is_dir() {
                if path.extension().is_some_and(|e| e == "app" || e == "appex") {
                    let info = plist::Value::from_file(path.join("Info.plist"))?;
                    let bundle = plist_text(&info, "CFBundleIdentifier")?;
                    let original = verified
                        .iter()
                        .find(|v| v["bundle_id"] == bundle)
                        .context("IPA contains an unverified application bundle")?;
                    let executable = plist_text(&info, "CFBundleExecutable")?;
                    crate::config::filename(executable)?;
                    ensure!(
                        seen.insert(bundle.to_owned())
                            && original["version"]
                                == plist_text(&info, "CFBundleShortVersionString")?
                            && original["build_number"] == plist_text(&info, "CFBundleVersion")?
                            && original["executable_sha256"].as_str()
                                == Some(&fsutil::sha256(&path.join(executable))?),
                        "IPA bundle differs from independent archive verification"
                    );
                    let mut record = original.clone();
                    record["path"] = json!(
                        path.strip_prefix(root)?
                            .to_str()
                            .context("Invalid IPA path")?
                    );
                    applications.push(record);
                }
                pending.push(path);
            } else {
                let parent = path.parent().unwrap();
                let framework = parent.extension().is_some_and(|e| e == "framework")
                    && path.file_name() == parent.file_stem();
                if path.extension().is_some_and(|e| e == "dylib") || framework {
                    embedded.push(json!({"path":path.strip_prefix(root)?.to_str().context("Invalid IPA path")?,"sha256":fsutil::sha256(&path)?}));
                }
            }
        }
    }
    ensure!(
        seen.len() == verified.len(),
        "IPA is missing verified bundles"
    );
    // Main app first, then sorted extensions: stable IDs and the deployed runtime SBOM contract.
    applications.sort_by_key(|v| {
        (
            v["bundle_id"] != app.config["app_store"]["bundle_id"],
            v["path"].as_str().unwrap().to_owned(),
        )
    });
    embedded.sort_by_key(|v| v["path"].as_str().unwrap().to_owned());
    let declared = app
        .config
        .get("runtime_dependencies")
        .cloned()
        .unwrap_or(json!([]));
    for dependency in declared
        .as_array()
        .context("runtime_dependencies must be a list")?
    {
        for key in ["name", "version", "source"] {
            ensure!(
                dependency[key]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
                "Declared runtime dependency {key} missing"
            );
        }
    }
    Ok(
        json!({"schema_version":1,"ipa_sha256":archive["ipa_sha256"],"applications":applications,
        "embedded_components":embedded,"declared_runtime_dependencies":declared,"runtime_dependency_policy":POLICY}),
    )
}

pub fn runtime_document(repository: &str, inventory: &Value, mut document: Value) -> Result<Value> {
    spdx(&document)?;
    document["name"] = json!("application.ipa");
    document["documentNamespace"] = json!(format!(
        "https://github.com/{repository}/sbom/{}",
        inventory["ipa_sha256"]
            .as_str()
            .context("IPA hash missing")?
    ));
    if document.get("relationships").is_none() {
        document["relationships"] = json!([]);
    }
    for (collection, prefix, purpose, relationship) in [
        (
            "applications",
            "ApplicationBundle",
            Some("APPLICATION"),
            "DESCRIBES",
        ),
        (
            "embedded_components",
            "Embedded",
            Some("LIBRARY"),
            "CONTAINS",
        ),
        (
            "declared_runtime_dependencies",
            "DeclaredDependency",
            None,
            "DEPENDS_ON",
        ),
    ] {
        for (index, item) in inventory[collection]
            .as_array()
            .context("Runtime inventory collection missing")?
            .iter()
            .enumerate()
        {
            let id = format!("SPDXRef-{prefix}-{index}");
            let mut package = json!({"SPDXID":id,"downloadLocation":"NOASSERTION","filesAnalyzed":false,
                "licenseConcluded":"NOASSERTION","copyrightText":"NOASSERTION"});
            if let Some(purpose) = purpose {
                package["primaryPackagePurpose"] = json!(purpose);
            }
            match collection {
                "applications" => {
                    package["name"] = item["bundle_id"].clone();
                    package["versionInfo"] = json!(format!(
                        "{}+{}",
                        item["version"].as_str().context("Bundle version missing")?,
                        item["build_number"]
                            .as_str()
                            .context("Bundle number missing")?
                    ));
                    package["checksums"] =
                        json!([{"algorithm":"SHA256","checksumValue":item["executable_sha256"]}]);
                }
                "embedded_components" => {
                    package["name"] = item["path"].clone();
                    package["checksums"] =
                        json!([{"algorithm":"SHA256","checksumValue":item["sha256"]}]);
                }
                _ => {
                    package["name"] = item["name"].clone();
                    package["versionInfo"] = item["version"].clone();
                    package["downloadLocation"] = item["source"].clone();
                    package["comment"] =
                        json!("Consumer-declared runtime dependency; may be statically linked.");
                }
            }
            document["packages"].as_array_mut().unwrap().push(package);
            document["relationships"].as_array_mut().context("SPDX relationships invalid")?.push(json!({
                "spdxElementId":if collection == "applications" { "SPDXRef-DOCUMENT" } else { "SPDXRef-ApplicationBundle-0" },
                "relatedSpdxElement":id,"relationshipType":relationship}));
        }
    }
    document["comment"] = json!(format!(
        "{POLICY}{}",
        if inventory["embedded_components"]
            .as_array()
            .context("Embedded inventory missing")?
            .is_empty()
        {
            " No embedded runtime libraries were found."
        } else {
            " Embedded components are inventoried without inferring their licenses or provenance."
        }
    ));
    spdx(&document)?;
    Ok(document)
}

pub fn generate(app: &App, executor: &mut impl Executor) -> Result<Value> {
    let output = app.root.join("build/sbom");
    fsutil::confined(&app.root, &output)?;
    // A failed scan must not leave a prior green report available to seal.
    if output.exists() {
        fs::remove_dir_all(&output)?;
    }
    let archive_path = app.root.join("build/rust-archive.json");
    let archive = read(&archive_path)?;
    let ipa = app.root.join("build/application.ipa");
    fsutil::confined(&app.root, &ipa)?;
    let archive_hash = fsutil::sha256(&archive_path)?;
    let inputs = crate::source::fingerprint(&app.root)?;
    ensure!(
        archive["inputs_sha256"] == inputs
            && archive["ipa_sha256"].as_str() == Some(&fsutil::sha256(&ipa)?),
        "Source or IPA changed after independent archive verification"
    );
    let tool: Value = serde_json::from_str(&checked(
        executor,
        &Step::new("syft", ["version", "--output", "json"], 30),
        &app.root,
    )?)?;
    ensure!(
        tool["version"] == SYFT_VERSION,
        "Install reviewed Syft {SYFT_VERSION} before generating release SBOMs"
    );
    fs::create_dir_all(app.root.join("build"))?;
    let staged = tempfile::tempdir_in(app.root.join("build"))?;
    let unpacked = tempfile::tempdir()?;
    extract(&ipa, unpacked.path())?;
    let inventory = inventory(app, unpacked.path(), &archive)?;
    let config = staged.path().join("syft.yaml");
    fs::write(&config, "check-for-app-update: false\n")?;
    for (name, source) in [
        ("source.spdx.json", "dir:.".to_owned()),
        (
            "ipa-syft.spdx.json",
            format!("dir:{}", unpacked.path().display()),
        ),
    ] {
        let mut args = vec![
            "scan".to_owned(),
            source,
            "--config".into(),
            config.to_string_lossy().into_owned(),
            "--output".into(),
            "spdx-json".into(),
        ];
        if name.starts_with("source") {
            for exclude in ["./.git", "./build", "./node_modules", "./vendor"] {
                args.extend(["--exclude".into(), exclude.into()]);
            }
        }
        let raw = checked(executor, &Step::new("syft", args, 600), &app.root)?;
        let document: Value = serde_json::from_str(&raw)?;
        spdx(&document)?;
        fsutil::json(&staged.path().join(name), &document)?;
    }
    let runtime = runtime_document(
        crate::github::repository(app)?,
        &inventory,
        read(&staged.path().join("ipa-syft.spdx.json"))?,
    )?;
    fsutil::json(&staged.path().join("ipa.spdx.json"), &runtime)?;
    ensure!(
        archive_hash == fsutil::sha256(&archive_path)?
            && inputs == crate::source::fingerprint(&app.root)?
            && archive["ipa_sha256"].as_str() == Some(&fsutil::sha256(&ipa)?),
        "Source or IPA changed during SBOM generation"
    );
    let report = json!({"schema_version":1,"generator":{"name":"syft","version":SYFT_VERSION},"repository":crate::github::repository(app)?,
        "source_sha":archive["source_sha"],"run_id":archive["run_id"],"inputs_sha256":inputs,
        "ipa_sha256":archive["ipa_sha256"],"archive_sha256":archive_hash,"inventory":inventory,
        "files":{"source.spdx.json":fsutil::sha256(&staged.path().join("source.spdx.json"))?,"ipa.spdx.json":fsutil::sha256(&staged.path().join("ipa.spdx.json"))?}});
    fsutil::json(&staged.path().join("sbom.json"), &report)?;
    fs::rename(staged.path(), output)?;
    Ok(report)
}

pub fn verify(root: &Path, archive_path: &Path, repository: &str) -> Result<()> {
    let report = read(&root.join("sbom.json"))?;
    let archive = read(archive_path)?;
    ensure!(
        report["schema_version"] == 1
            && report["generator"] == json!({"name":"syft","version":SYFT_VERSION})
            && report["repository"] == repository
            && report["archive_sha256"].as_str() == Some(&fsutil::sha256(archive_path)?),
        "SBOM belongs to another archive or generator"
    );
    for key in ["source_sha", "run_id", "inputs_sha256", "ipa_sha256"] {
        ensure!(
            report[key] == archive[key],
            "SBOM {key} differs from verified archive"
        );
    }
    for name in &FILES[..2] {
        let path = root.join(name);
        spdx(&read(&path)?)?;
        ensure!(
            report["files"][name].as_str() == Some(&fsutil::sha256(&path)?),
            "SBOM changed after generation: {name}"
        );
    }
    Ok(())
}

pub fn attach(app: &App, target: &Path) -> Result<()> {
    let source = app.root.join("build/sbom");
    fsutil::confined(&app.root, &source)?;
    verify(
        &source,
        &app.root.join("build/rust-archive.json"),
        crate::github::repository(app)?,
    )?;
    for name in FILES {
        fs::copy(source.join(name), target.join(name))?;
    }
    Ok(())
}
