use crate::{
    config::{App, filename},
    fsutil,
    process::{Executor, Step, checked},
    toolchain,
};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path, time::SystemTime};

pub fn validate_numbers(version: &str, number: &str) -> Result<()> {
    ensure!(
        Regex::new(r"^\d+\.\d+\.\d+$")?.is_match(version)
            && Regex::new(r"^[1-9]\d{0,3}\.[1-9]\d?$")?.is_match(number),
        "Invalid version/build number"
    );
    Ok(())
}

pub fn options(app: &App) -> Result<plist::Value> {
    let mut profiles = plist::Dictionary::new();
    for target in app.config["targets"]
        .as_array()
        .context("Missing targets")?
    {
        profiles.insert(
            target["bundle_id"].as_str().unwrap().to_owned(),
            target["profile"].as_str().unwrap().into(),
        );
    }
    let mut values = plist::Dictionary::new();
    for (key, value) in [
        ("method", "app-store-connect"),
        ("signingStyle", "manual"),
        ("teamID", app.text("team_id")?),
    ] {
        values.insert(key.into(), value.into());
    }
    values.insert("manageAppVersionAndBuildNumber".into(), false.into());
    values.insert("uploadSymbols".into(), true.into());
    values.insert("provisioningProfiles".into(), profiles.into());
    Ok(values.into())
}

pub fn steps(app: &App, version: &str, number: &str) -> Result<Vec<Step>> {
    validate_numbers(version, number)?;
    let (kind, project) = app.project()?;
    let developer = app.xcode(false)?["path"]
        .as_str()
        .context("Missing Xcode path")?;
    let archive = Step::new(
        "xcodebuild",
        [
            format!("-{kind}"),
            project.into(),
            "-scheme".into(),
            app.text("scheme")?.into(),
            "-configuration".into(),
            "Release".into(),
            "-destination".into(),
            "generic/platform=iOS".into(),
            "-archivePath".into(),
            "build/application.xcarchive".into(),
            format!("MARKETING_VERSION={version}"),
            format!("CURRENT_PROJECT_VERSION={number}"),
            "clean".into(),
            "archive".into(),
        ],
        3600,
    )
    .developer(developer);
    let export = Step::new(
        "xcodebuild",
        [
            "-exportArchive",
            "-archivePath",
            "build/application.xcarchive",
            "-exportPath",
            "build/rust-export",
            "-exportOptionsPlist",
            "build/rust-export-options.plist",
        ],
        1800,
    )
    .developer(developer);
    Ok(vec![archive, export])
}

pub fn run(app: &App, version: &str, number: &str, executor: &mut impl Executor) -> Result<()> {
    let steps = steps(app, version, number)?;
    toolchain::resolve(app, false, &[], executor)?;
    let export = app.root.join("build/rust-export");
    for path in [
        "build",
        "build/application.xcarchive",
        "build/logs",
        "build/rust-export-options.plist",
        "build/application.ipa",
        "build/rust-archive.json",
    ] {
        fsutil::confined(&app.root, &app.root.join(path))?;
    }
    fsutil::confined(&app.root, &export)?;
    if export.exists() {
        fs::remove_dir_all(&export)?;
    }
    let ipa = app.root.join("build/application.ipa");
    if ipa.exists() {
        fs::remove_file(&ipa)?;
    }
    let report = app.root.join("build/rust-archive.json");
    if report.exists() {
        fs::remove_file(&report)?;
    }
    fs::create_dir_all(app.root.join("build"))?;
    options(app)?.to_file_xml(app.root.join("build/rust-export-options.plist"))?;
    for (index, step) in steps.iter().enumerate() {
        let output = executor.run(
            step,
            &app.root,
            Some(
                &app.root
                    .join(format!("build/logs/rust-archive-{index}.log")),
            ),
        )?;
        ensure!(
            output.status == 0,
            "Archive/export exited {}; inspect retained build logs",
            output.status
        );
    }
    let candidates: Vec<_> = fs::read_dir(&export)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ipa"))
        .collect();
    ensure!(candidates.len() == 1, "Expected one exported IPA");
    fs::rename(&candidates[0], &ipa)?;
    let inventory = verify(app, &ipa, version, number, executor)?;
    fsutil::json(
        &report,
        &json!({"schema_version":1,"implementation":"rust","local_archive":true,"source_sha":std::env::var("SOURCE_SHA").or_else(|_| std::env::var("GITHUB_SHA")).ok(),"version":version,"build_number":number,"ipa_sha256":fsutil::sha256(&ipa)?,"applications":inventory,"release_candidate":false}),
    )?;
    Ok(())
}

fn text<'a>(value: &'a plist::Value, key: &str) -> Result<&'a str> {
    value
        .as_dictionary()
        .and_then(|v| v.get(key))
        .and_then(plist::Value::as_string)
        .with_context(|| format!("Missing bundle field: {key}"))
}
fn bool_value(value: &plist::Value, key: &str) -> Option<bool> {
    value.as_dictionary()?.get(key)?.as_boolean()
}

pub fn uuid_set(output: &str) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for line in output.lines().filter(|v| v.starts_with("UUID:")) {
        let id = line
            .split_whitespace()
            .nth(1)
            .context("Malformed binary UUID")?;
        crate::simulator::validate_id(id)?;
        ids.insert(id.to_ascii_uppercase());
    }
    ensure!(!ids.is_empty(), "Missing binary UUIDs");
    Ok(ids)
}

fn symbols(
    path: &Path,
    developer: &str,
    root: &Path,
    executor: &mut impl Executor,
) -> Result<BTreeSet<String>> {
    uuid_set(&checked(
        executor,
        &Step::new(
            "xcrun",
            [
                "dwarfdump",
                "--uuid",
                path.to_str().context("Invalid binary path")?,
            ],
            30,
        )
        .developer(developer),
        root,
    )?)
}

pub fn verify(
    app: &App,
    ipa: &Path,
    version: &str,
    number: &str,
    executor: &mut impl Executor,
) -> Result<Vec<Value>> {
    let directory = tempfile::tempdir()?;
    let mut zip = zip::ZipArchive::new(fs::File::open(ipa)?)?;
    for index in 0..zip.len() {
        let file = zip.by_index(index)?;
        ensure!(
            file.enclosed_name().is_some() && !file.is_symlink(),
            "Unsafe IPA member"
        );
    }
    zip.extract(directory.path())?;
    let applications: Vec<_> = fs::read_dir(directory.path().join("Payload"))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
        .collect();
    ensure!(applications.len() == 1, "Expected one main application");
    let mut bundles = applications.clone();
    let plugins = applications[0].join("PlugIns");
    if plugins.exists() {
        bundles.extend(
            fs::read_dir(plugins)?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "appex")),
        );
    }
    let mut found = BTreeSet::new();
    let mut inventory = Vec::new();
    let developer = app.xcode(false)?["path"]
        .as_str()
        .context("Missing Xcode path")?;
    let mut available_symbols = BTreeSet::new();
    let dsym_root = app.root.join("build/application.xcarchive/dSYMs");
    fsutil::confined(&app.root, &dsym_root)?;
    for entry in fs::read_dir(dsym_root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|v| v == "dSYM") {
            available_symbols.extend(symbols(&path, developer, &app.root, executor)?);
        }
    }
    for bundle in bundles {
        checked(
            executor,
            &Step::new(
                "codesign",
                [
                    "--verify",
                    "--deep",
                    "--strict",
                    bundle.to_str().context("Invalid bundle path")?,
                ],
                60,
            ),
            &app.root,
        )?;
        let info = plist::Value::from_file(bundle.join("Info.plist"))?;
        let id = text(&info, "CFBundleIdentifier")?;
        let target = app.config["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["bundle_id"] == id)
            .context("Unexpected application bundle")?;
        ensure!(found.insert(id.to_owned()), "Duplicate application bundle");
        ensure!(
            text(&info, "CFBundleShortVersionString")? == version
                && text(&info, "CFBundleVersion")? == number,
            "Exported version/build differs"
        );
        ensure!(
            Some(text(&info, "DTXcodeBuild")?) == app.config["xcode"]["build"].as_str()
                && text(&info, "DTSDKName")?
                    == format!("iphoneos{}", app.config["xcode"]["sdk"].as_str().unwrap()),
            "Export used an unexpected Xcode/SDK"
        );
        ensure!(
            bool_value(&info, "ITSAppUsesNonExemptEncryption")
                == target["non_exempt_encryption"].as_bool(),
            "Missing or mismatched encryption policy"
        );
        let privacy = plist::Value::from_file(bundle.join("PrivacyInfo.xcprivacy"))?;
        ensure!(
            bool_value(&privacy, "NSPrivacyTracking") == target["tracking"].as_bool()
                && privacy
                    .as_dictionary()
                    .is_some_and(|p| p.contains_key("NSPrivacyAccessedAPITypes")),
            "Missing or invalid privacy manifest"
        );
        let profile = checked(
            executor,
            &Step::new(
                "security",
                [
                    "cms",
                    "-D",
                    "-i",
                    bundle
                        .join("embedded.mobileprovision")
                        .to_str()
                        .context("Invalid profile path")?,
                ],
                30,
            ),
            &app.root,
        )?;
        let profile = plist::Value::from_reader_xml(profile.as_bytes())?;
        let dict = profile
            .as_dictionary()
            .context("Malformed provisioning profile")?;
        ensure!(
            dict.get("ExpirationDate")
                .and_then(plist::Value::as_date)
                .is_some_and(|date| SystemTime::from(date) > SystemTime::now()),
            "Expired provisioning profile"
        );
        ensure!(
            !dict
                .get("ProvisionedDevices")
                .and_then(plist::Value::as_array)
                .is_some_and(|v| !v.is_empty())
                && bool_value(&profile, "ProvisionsAllDevices") != Some(true),
            "Not an App Store profile"
        );
        let entitlements = checked(
            executor,
            &Step::new(
                "codesign",
                [
                    "--display",
                    "--entitlements",
                    ":-",
                    bundle.to_str().unwrap(),
                ],
                30,
            ),
            &app.root,
        )?;
        let entitlements = plist::Value::from_reader_xml(entitlements.as_bytes())?;
        ensure!(
            text(&entitlements, "application-identifier")?
                == format!("{}.{id}", app.text("team_id")?)
                && text(&entitlements, "com.apple.developer.team-identifier")?
                    == app.text("team_id")?
                && bool_value(&entitlements, "get-task-allow") != Some(true),
            "Invalid signing identity/debug entitlement"
        );
        let expected: Value = serde_json::to_value(&entitlements)?;
        for (key, value) in target["entitlements"].as_object().unwrap() {
            ensure!(
                expected.get(key) == Some(value),
                "Entitlement mismatch for {id}: {key}"
            );
        }
        ensure!(
            dict.get("Entitlements")
                .and_then(plist::Value::as_dictionary)
                .and_then(|v| v.get("application-identifier"))
                .and_then(plist::Value::as_string)
                == Some(text(&entitlements, "application-identifier")?),
            "Profile identity mismatch"
        );
        let executable_name = text(&info, "CFBundleExecutable")?;
        filename(executable_name)?;
        let executable = bundle.join(executable_name);
        let uuids = symbols(&executable, developer, &app.root, executor)?;
        ensure!(
            uuids.is_subset(&available_symbols),
            "Missing matching dSYMs"
        );
        let prefix = directory
            .path()
            .join(format!("certificate-{}-", inventory.len()));
        checked(
            executor,
            &Step::new(
                "codesign",
                [
                    "--display".to_owned(),
                    format!("--extract-certificates={}", prefix.display()),
                    bundle.to_string_lossy().into_owned(),
                ],
                30,
            ),
            &app.root,
        )?;
        let leaf = fs::read(format!("{}0", prefix.display()))?;
        ensure!(
            dict.get("DeveloperCertificates")
                .and_then(plist::Value::as_array)
                .is_some_and(|certificates| certificates
                    .iter()
                    .any(|cert| cert.as_data() == Some(leaf.as_slice()))),
            "Signing certificate is not authorized by the embedded profile"
        );
        inventory.push(json!({"bundle_id":id,"version":version,"build_number":number,"profile_uuid":text(&profile,"UUID")?,"profile_sha256":fsutil::sha256(&bundle.join("embedded.mobileprovision"))?,"executable_sha256":fsutil::sha256(&executable)?,"binary_uuids":uuids,"certificate_sha256":fsutil::sha256(Path::new(&format!("{}0", prefix.display())))?}));
    }
    let expected: BTreeSet<_> = app.config["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["bundle_id"].as_str().unwrap().to_owned())
        .collect();
    ensure!(found == expected, "Missing application or extension in IPA");
    Ok(inventory)
}
