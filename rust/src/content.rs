//! Immutable store inputs carried by a native preparation, independent of later checkouts.
use crate::{
    config::{App, relative_path},
    fsutil, metadata,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, io::Read, path::Path};

pub const CORE: [&str; 4] = ["application.ipa", "app.json", "qa.json", "archive.json"];
pub const TEXT_FIELDS: [&str; 9] = [
    "description.txt",
    "keywords.txt",
    "support_url.txt",
    "release_notes.txt",
    "promotional_text.txt",
    "marketing_url.txt",
    "name.txt",
    "subtitle.txt",
    "privacy_url.txt",
];

/// Copy only public, supported store fields. Never recursively copy credentials or signing inputs.
pub fn snapshot(app: &App, target: &Path) -> Result<Value> {
    let locales = metadata::preflight(app)?;
    let mut config = app.config.clone();
    config["metadata_path"] = json!("store/metadata");
    config["screenshots_path"] = json!("store/screenshots");
    for (locale, _, _) in locales {
        for field in TEXT_FIELDS {
            let source = app.path("metadata_path")?.join(&locale).join(field);
            if source.exists() {
                copy(
                    &app.root,
                    &source,
                    &target.join("store/metadata").join(&locale).join(field),
                )?;
            }
        }
        for source in metadata::screenshot_files(app, &locale)? {
            copy(
                &app.root,
                &source,
                &target
                    .join("store/screenshots")
                    .join(&locale)
                    .join(source.file_name().unwrap()),
            )?;
        }
    }
    if app.config["accessibility_path"].as_str().is_some() {
        copy(
            &app.root,
            &app.path("accessibility_path")?,
            &target.join("store/accessibility.json"),
        )?;
        config["accessibility_path"] = json!("store/accessibility.json");
    }
    if let Some(review) = metadata::review_document(app)? {
        fsutil::json(&target.join("store/review.json"), &review)?;
    }
    Ok(config)
}

fn copy(root: &Path, source: &Path, target: &Path) -> Result<()> {
    fsutil::confined(root, source)?;
    ensure!(
        source.is_file() && !source.is_symlink(),
        "Store input must be a regular file: {}",
        source.display()
    );
    ensure!(
        fs::metadata(source)?.len() <= 32 * 1024 * 1024,
        "Store input exceeds 32 MiB"
    );
    fsutil::atomic(target, &fs::read(source)?)
}

pub fn inventory(root: &Path) -> Result<Value> {
    let mut files = json!({});
    let mut pending = vec![root.to_owned()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            ensure!(
                !entry.file_type()?.is_symlink(),
                "Release input is a symlink"
            );
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                let name = entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("Non-UTF8 release path")?
                    .to_owned();
                validate_name(&name)?;
                files[&name] = json!(fsutil::sha256(&entry.path())?);
            }
        }
    }
    ensure!(
        files.as_object().unwrap().len() <= 10000,
        "Release contains too many files"
    );
    Ok(files)
}

fn validate_name(name: &str) -> Result<()> {
    relative_path(name)?;
    ensure!(
        !name.contains('\\') && name.split('/').all(|s| !s.is_empty() && s != "."),
        "Noncanonical release path"
    );
    let parts: Vec<_> = name.split('/').collect();
    let store = matches!(parts.as_slice(), ["store", "metadata", _, field] if TEXT_FIELDS.contains(field))
        || matches!(parts.as_slice(), ["store", "screenshots", _, file] if file.ends_with(".png"))
        || matches!(name, "store/accessibility.json" | "store/review.json");
    ensure!(
        CORE.contains(&name)
            || store
            || matches!(
                name,
                "symbols.zip" | "source.spdx.json" | "ipa.spdx.json" | "version.json"
            ),
        "Unexpected release input: {name}"
    );
    Ok(())
}

pub fn names(manifest: &Value) -> Result<Vec<String>> {
    if manifest["schema_version"] == 2 {
        return Ok(CORE
            .iter()
            .map(|s| (*s).to_owned())
            .chain(["release.json".into()])
            .collect());
    }
    ensure!(
        manifest["schema_version"] == 3,
        "Unsupported native release format"
    );
    let files = manifest["files"]
        .as_object()
        .context("Release file manifest missing")?;
    ensure!(
        files.len() <= 10000
            && files.contains_key("symbols.zip")
            && CORE.iter().all(|s| files.contains_key(*s)),
        "Incomplete or oversized release file manifest"
    );
    for (name, hash) in files {
        validate_name(name)?;
        ensure!(
            hash.as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())),
            "Invalid release checksum"
        );
    }
    Ok(files
        .keys()
        .cloned()
        .chain(["release.json".into()])
        .collect())
}

pub fn verify(root: &Path, manifest: &Value) -> Result<()> {
    for name in names(manifest)? {
        let path = root.join(&name);
        fsutil::confined(root, &path)?;
        ensure!(
            path.is_file() && !path.is_symlink(),
            "Release file missing or not regular: {name}"
        );
        if manifest["schema_version"] == 3 && name != "release.json" {
            ensure!(
                manifest["files"][&name].as_str() == Some(&fsutil::sha256(&path)?),
                "Release content changed: {name}"
            );
        }
    }
    Ok(())
}

/// Only call after authenticating the package. Extract atomically, with path/count/size bounds.
pub fn unpack(app: &App, package: &Path, output: &Path) -> Result<()> {
    fsutil::confined(&app.root, output)?;
    ensure!(!output.exists(), "Release directory already exists");
    let mut archive = zip::ZipArchive::new(fs::File::open(package)?)?;
    let manifest: Value = {
        let mut file = archive.by_name("release.json")?;
        ensure!(file.size() <= 4 * 1024 * 1024, "Oversized release manifest");
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        serde_json::from_slice(&data)?
    };
    let allowed = names(&manifest)?.into_iter().collect::<BTreeSet<_>>();
    let actual = archive
        .file_names()
        .map(String::from)
        .collect::<BTreeSet<_>>();
    ensure!(
        actual == allowed && archive.len() == allowed.len(),
        "Release package has missing, duplicate or unexpected files"
    );
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        ensure!(
            !entry.is_symlink() && entry.size() <= 4 * 1024 * 1024 * 1024,
            "Unsafe release entry"
        );
        total = total
            .checked_add(entry.size())
            .context("Release size overflow")?;
        ensure!(total <= 8 * 1024 * 1024 * 1024, "Release exceeds 8 GiB");
    }
    fs::create_dir_all(output.parent().context("Release needs a parent")?)?;
    let stage = tempfile::tempdir_in(output.parent().unwrap())?;
    for name in allowed {
        let mut entry = archive.by_name(&name)?;
        let path = stage.path().join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        std::io::copy(&mut entry, &mut file)?;
        file.sync_all()?;
    }
    crate::store::Release::load(app, stage.path())?;
    fs::rename(stage.path(), output)?;
    Ok(())
}
