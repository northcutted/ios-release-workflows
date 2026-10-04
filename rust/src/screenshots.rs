use crate::{
    config::{App, relative_path},
    fsutil,
    process::{Executor, Step, checked},
    results, simulator, toolchain,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Options {
    pub scheme: String,
    pub test_target: String,
    pub devices: Vec<String>,
    pub languages: Vec<String>,
    pub only_testing: Vec<String>,
    pub output: PathBuf,
    pub photos: Vec<PathBuf>,
    pub videos: Vec<PathBuf>,
    pub isolated_cache: bool,
}

impl Options {
    pub fn validate(&self, app: &App) -> Result<()> {
        ensure!(
            !self.scheme.is_empty() && !self.test_target.is_empty(),
            "Screenshot scheme and test target must be explicit"
        );
        relative_path(self.output.to_str().context("Invalid screenshot output")?)?;
        ensure!(
            self.output.starts_with("build")
                && self.output.components().count() >= 3
                && self.output != Path::new("build/rust-screenshots"),
            "Raw screenshot output must be a directory below build; source assets are never overwritten"
        );
        ensure!(
            !self.devices.is_empty() && !self.languages.is_empty(),
            "Screenshot devices/locales must be explicit"
        );
        for (requested, configured) in [
            (&self.devices, app.names("screenshot_devices")?),
            (&self.languages, app.names("locales")?),
        ] {
            ensure!(
                requested.iter().all(|v| configured.contains(v))
                    && requested
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == requested.len(),
                "Unknown or duplicate screenshot selection"
            );
        }
        ensure!(self.only_testing.iter().all(|s| s == &self.test_target || s.starts_with(&format!("{}/",self.test_target))), "Screenshot tests must belong to the explicit target");
        for path in self.photos.iter().chain(&self.videos) {
            relative_path(path.to_str().context("Invalid media path")?)?;
        }
        Ok(())
    }
}

pub fn build_step(app: &App, options: &Options, id: &str, products: &Path) -> Result<Step> {
    let (kind, project) = app.project()?;
    let args = vec![
        format!("-{kind}"),
        project.into(),
        "-scheme".into(),
        options.scheme.clone(),
        "-configuration".into(),
        app.configuration("test").into(),
        "-sdk".into(),
        "iphonesimulator".into(),
        "-destination".into(),
        format!("platform=iOS Simulator,id={id}"),
        "-derivedDataPath".into(),
        products.to_string_lossy().into_owned(),
        "-disableAutomaticPackageResolution".into(),
        "-skipPackageUpdates".into(),
        "CODE_SIGN_IDENTITY=".into(),
        "CODE_SIGNING_REQUIRED=NO".into(),
        "CODE_SIGNING_ALLOWED=NO".into(),
        "build-for-testing".into(),
    ];
    Ok(Step::new("xcodebuild", args, 2400).developer(
        app.xcode(false)?["path"]
            .as_str()
            .context("Missing Xcode path")?,
    ))
}

pub fn test_step(
    app: &App,
    options: &Options,
    id: &str,
    products: &Path,
    bundle: &Path,
) -> Result<Step> {
    let mut args = vec![
        "test-without-building".into(),
        "-xctestrun".into(),
        products.to_string_lossy().into_owned(),
        "-destination".into(),
        format!("platform=iOS Simulator,id={id}"),
        "-parallel-testing-enabled".into(),
        "NO".into(),
        "-parallel-testing-worker-count".into(),
        "1".into(),
        "-maximum-concurrent-test-simulator-destinations".into(),
        "1".into(),
        "-collect-test-diagnostics".into(),
        "on-failure".into(),
        "-resultBundlePath".into(),
        bundle.to_string_lossy().into_owned(),
    ];
    let selected = if options.only_testing.is_empty() {
        vec![options.test_target.clone()]
    } else {
        options.only_testing.clone()
    };
    args.extend(selected.iter().map(|name| format!("-only-testing:{name}")));
    Ok(Step::new("xcodebuild", args, 2400).developer(
        app.xcode(false)?["path"]
            .as_str()
            .context("Missing Xcode path")?,
    ))
}

pub fn configure_test_run(path: &Path, target: &str, home: &Path) -> Result<()> {
    fn walk(value: &mut plist::Value, target: &str, home: &str, count: &mut usize) -> Result<()> {
        if let Some(dict) = value.as_dictionary_mut() {
            if dict.get("BlueprintName").and_then(plist::Value::as_string) == Some(target)
                || dict.get("TestTargetName").and_then(plist::Value::as_string) == Some(target)
            {
                for key in ["EnvironmentVariables", "TestingEnvironmentVariables"] {
                    if !dict.contains_key(key) {
                        dict.insert(key.into(), plist::Dictionary::new().into());
                    }
                    let vars = dict
                        .get_mut(key)
                        .and_then(plist::Value::as_dictionary_mut)
                        .context("Malformed XCTest environment")?;
                    vars.insert("IOS_RELEASE_SNAPSHOT_HOME".into(), home.into());
                }
                *count += 1;
            }
            // Xcode's legacy v1 format names the target by the dictionary key.
            if let Some(node) = dict
                .get_mut(target)
                .and_then(plist::Value::as_dictionary_mut)
            {
                for key in ["EnvironmentVariables", "TestingEnvironmentVariables"] {
                    if !node.contains_key(key) {
                        node.insert(key.into(), plist::Dictionary::new().into());
                    }
                    node.get_mut(key)
                        .and_then(plist::Value::as_dictionary_mut)
                        .context("Malformed XCTest environment")?
                        .insert("IOS_RELEASE_SNAPSHOT_HOME".into(), home.into());
                }
                *count += 1;
            }
            for child in dict.values_mut() {
                walk(child, target, home, count)?;
            }
        } else if let Some(items) = value.as_array_mut() {
            for item in items {
                walk(item, target, home, count)?;
            }
        }
        Ok(())
    }
    let mut value = plist::Value::from_file(path)?;
    let mut matches = 0;
    walk(
        &mut value,
        target,
        home.to_str().context("Invalid screenshot cache path")?,
        &mut matches,
    )?;
    ensure!(
        matches > 0,
        "The compiled test run does not contain the screenshot target"
    );
    value.to_file_xml(path)?;
    Ok(())
}

pub fn run(app: &App, options: &Options, executor: &mut impl Executor) -> Result<()> {
    options.validate(app)?;
    ensure!(
        !app.names("screens")?.is_empty(),
        "Configure the screenshot names produced by your UI tests; an empty capture cannot verify store assets"
    );
    ensure!(
        options.isolated_cache
            || (std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")),
        "Legacy SnapshotHelper capture requires a disposable GitHub-hosted runner; --isolated-cache requires a helper that reads IOS_RELEASE_SNAPSHOT_HOME"
    );
    let output = app.root.join(&options.output);
    fsutil::confined(&app.root, &app.root.join("build"))?;
    fsutil::confined(&app.root, &output)?;
    let resolved = toolchain::resolve(app, false, &options.devices, executor)?;
    fsutil::json(&app.root.join("build/build-env.json"), &resolved.evidence)?;
    let job = app
        .root
        .join("build/rust-screenshots")
        .join(uuid::Uuid::new_v4().to_string());
    fsutil::confined(&app.root, &job)?;
    fsutil::confined(&app.root, &output)?;
    fs::create_dir_all(&job)?;
    let legacy_cache = if options.isolated_cache {
        None
    } else {
        Some(LegacyCache::claim(
            Path::new(&std::env::var_os("HOME").context("Missing host home")?),
            &job.join("legacy-snapshot-cache"),
        )?)
    };
    // A failed run must not expose previous captures as current output. Retain
    // them in the evidence directory rather than deleting them.
    if output.exists() {
        fs::rename(&output, job.join("previous-images"))?;
    }
    let staging = job.join("images");
    let mut recovery_used = false;
    let mut records = Vec::new();
    for device in &options.devices {
        let id = resolved
            .devices
            .get(device)
            .context("Missing screenshot device")?;
        let home = job.join(id).join("host-home");
        let cache = legacy_cache
            .as_ref()
            .map(|cache| cache.path.clone())
            .unwrap_or_else(|| home.join("Library/Caches/tools.fastlane"));
        fs::create_dir_all(cache.join("screenshots"))?;
        let products = job.join(id).join("derived-data");
        simulator::prepare(
            id,
            &resolved.developer,
            &app.root,
            &job.join(id).join("readiness-build.json"),
            executor,
        )?;
        let built = executor.run(
            &build_step(app, options, id, &products)?,
            &app.root,
            Some(&job.join(id).join("build.log")),
        )?;
        ensure!(
            built.status == 0,
            "Screenshot build failed; inspect {}",
            job.display()
        );
        let candidates: Vec<_> = fs::read_dir(products.join("Build/Products"))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|v| v == "xctestrun"))
            .collect();
        ensure!(
            candidates.len() == 1,
            "Expected exactly one compiled screenshot test run"
        );
        let test_run = &candidates[0];
        configure_test_run(test_run, &options.test_target, &home)?;
        configure_capture_device(test_run, device)?;
        for locale in &options.languages {
            fsutil::atomic(&cache.join("language.txt"), locale.as_bytes())?;
            fsutil::atomic(&cache.join("locale.txt"), locale.as_bytes())?;
            fsutil::atomic(&cache.join("snapshot-launch_arguments.txt"), b"")?;
            let captures = cache.join("screenshots");
            for file in fs::read_dir(&captures)? {
                let file = file?.path();
                if file.is_file() {
                    fs::remove_file(file)?;
                }
            }
            let evidence = job.join(id).join(locale);
            fs::create_dir_all(&evidence)?;
            let mut success = false;
            let mut recovery_record = RecoveryRecord(None);
            for attempt in 0..2 {
                let bundle = evidence.join(format!("attempt-{attempt}.xcresult"));
                let log = evidence.join(format!("attempt-{attempt}.log"));
                simulator::prepare(
                    id,
                    &resolved.developer,
                    &app.root,
                    &evidence.join(format!("readiness-{attempt}.json")),
                    executor,
                )?;
                seed_media(app, options, id, &resolved.developer, executor)?;
                let status = executor
                    .run(
                        &test_step(app, options, id, test_run, &bundle)?,
                        &app.root,
                        Some(&log),
                    )?
                    .status;
                if status == 0 {
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
                                    bundle.to_str().unwrap(),
                                    "--compact",
                                ],
                                30,
                            )
                            .developer(&resolved.developer),
                            &app.root,
                        )?;
                        fsutil::atomic(&evidence.join(format!("{report}.json")), raw.as_bytes())?;
                        reports.push(serde_json::from_str::<Value>(&raw)?);
                    }
                    let report = results::junit(&reports[0], &reports[1])?;
                    ensure!(report.passed, "Screenshot test result did not pass");
                    fsutil::atomic(&evidence.join("report.junit"), report.xml.as_bytes())?;
                    success = true;
                    if let Some(record) = recovery_record.0.take() {
                        fsutil::json(
                            &record,
                            &json!({"status":"recovered","udid":id,"reuse_build":true}),
                        )?;
                        records.push(json!({"device":device,"locale":locale,"tests":report.executed,"recovered":true}));
                    } else {
                        records.push(json!({"device":device,"locale":locale,"tests":report.executed,"recovered":false}));
                    }
                    break;
                }
                let recognized = simulator::inspect(
                    &bundle,
                    &resolved.developer,
                    &app.root,
                    &evidence.join(format!("assessment-{attempt}.json")),
                    executor,
                )?;
                let hosted = std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                    && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted");
                ensure!(
                    status == 65
                        && hosted
                        && recognized
                        && attempt == 0
                        && !recovery_used
                        && test_run.is_file(),
                    "Screenshot execution failed; recovery refused. Evidence: {}",
                    evidence.display()
                );
                recovery_used = true;
                let record = evidence.join("recovery.json");
                fsutil::json(
                    &record,
                    &json!({"status":"retrying","udid":id,"reuse_build":true}),
                )?;
                recovery_record.0 = Some(record);
                simulator::reset(id, id, &resolved.developer, &app.root, executor)?;
                // Rebuild is deliberately absent: the next attempt uses the same xctestrun.
            }
            ensure!(success, "Screenshot tests did not succeed");
            let locale_output = staging.join(locale);
            fs::create_dir_all(&locale_output)?;
            for screen in app.names("screens")? {
                let filename = format!("{device}-{screen}.png");
                let source = captures.join(&filename);
                ensure!(
                    source.is_file(),
                    "Missing screenshot {filename}; refusing stale output"
                );
                validate_png(app, device, &source)?;
                fsutil::atomic(
                    &locale_output.join(filename),
                    &crate::images::store_png(&fs::read(source)?)?,
                )?;
            }
        }
    }
    fsutil::json(
        &job.join("capture.json"),
        &json!({"schema_version":1,"captures":records,"cache_mode":if options.isolated_cache {"isolated"} else {"legacy-host"},"source_sha":std::env::var("SOURCE_SHA").or_else(|_|std::env::var("GITHUB_SHA")).ok()}),
    )?;
    fs::create_dir_all(
        output
            .parent()
            .context("Missing screenshot output parent")?,
    )?;
    fs::rename(&staging, &output)?;
    println!(
        "{}",
        json!({"output":output,"evidence":job,"captures":records})
    );
    Ok(())
}

pub fn configure_capture_device(path: &Path, device: &str) -> Result<()> {
    fn walk(value: &mut plist::Value, device: &str) {
        if let Some(dict) = value.as_dictionary_mut() {
            if dict.contains_key("IOS_RELEASE_SNAPSHOT_HOME") {
                dict.insert("IOS_RELEASE_SNAPSHOT_DEVICE".into(), device.into());
            }
            for value in dict.values_mut() {
                walk(value, device);
            }
        } else if let Some(list) = value.as_array_mut() {
            for value in list {
                walk(value, device);
            }
        }
    }
    let mut value = plist::Value::from_file(path)?;
    walk(&mut value, device);
    value.to_file_xml(path)?;
    Ok(())
}

fn seed_media(
    app: &App,
    options: &Options,
    id: &str,
    developer: &str,
    executor: &mut impl Executor,
) -> Result<()> {
    for path in options.photos.iter().chain(&options.videos) {
        let file = app.root.join(path);
        fsutil::confined(&app.root, &file)?;
        ensure!(file.is_file(), "Missing media seed");
        checked(
            executor,
            &Step::new(
                "xcrun",
                [
                    "simctl",
                    "addmedia",
                    id,
                    file.to_str().context("Invalid media path")?,
                ],
                60,
            )
            .developer(developer),
            &app.root,
        )?;
    }
    Ok(())
}

struct RecoveryRecord(Option<PathBuf>);
impl Drop for RecoveryRecord {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let mut record = fs::read(path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                .unwrap_or_else(|| json!({"reuse_build":true}));
            record["status"] = json!("failed");
            let _ = fsutil::json(path, &record);
        }
    }
}

// XCTest controls SIMULATOR_HOST_HOME itself. Existing SnapshotHelper versions
// therefore require the real host cache. Claim it only on a disposable host,
// refuse existing data, and retain this job's directory on every exit path.
pub struct LegacyCache {
    pub path: PathBuf,
    retained: PathBuf,
}
impl LegacyCache {
    pub fn claim(home: &Path, retained: &Path) -> Result<Self> {
        let home = home.canonicalize()?;
        let parent = home.join("Library/Caches");
        fsutil::confined(&home, &parent)?;
        fs::create_dir_all(&parent)?;
        ensure!(
            parent.canonicalize()?.starts_with(&home),
            "Host cache escapes the host home"
        );
        let path = parent.join("tools.fastlane");
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).context(
            "SnapshotHelper cache already exists; refusing to alter another capture's files",
        )?;
        Ok(Self {
            path,
            retained: retained.to_owned(),
        })
    }
}
impl Drop for LegacyCache {
    fn drop(&mut self) {
        if let Err(error) = fs::rename(&self.path, &self.retained) {
            eprintln!("Could not retain owned SnapshotHelper cache: {error}");
        }
    }
}

pub fn validate_png(app: &App, device: &str, path: &Path) -> Result<()> {
    use std::io::Read;
    let mut header = [0_u8; 24];
    fs::File::open(path)?.read_exact(&mut header)?;
    ensure!(
        &header[..8] == b"\x89PNG\r\n\x1a\n" && &header[12..16] == b"IHDR",
        "Invalid PNG screenshot"
    );
    let size = [
        u32::from_be_bytes(header[16..20].try_into()?),
        u32::from_be_bytes(header[20..24].try_into()?),
    ];
    let classes = app.config["screenshot_classes"]
        .as_object()
        .context("Missing screenshot dimensions policy")?;
    let matching: Vec<_> = classes
        .values()
        .filter(|v| {
            v["names"]
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == device))
        })
        .collect();
    ensure!(
        matching.len() == 1
            && matching[0]["sizes"]
                .as_array()
                .is_some_and(|sizes| sizes.iter().any(|value| value == &json!(size))),
        "Screenshot dimensions differ from configured device class"
    );
    Ok(())
}
