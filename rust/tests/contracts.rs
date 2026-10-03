use anyhow::Result;
use ios_release_native::{
    archive,
    config::{App, filename, relative_path},
    fsutil, localization,
    process::{Arg, Executor, Native, Output, Step},
    qa, results, screenshots, simulator, toolchain,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const ID: &str = "C71FB2C5-952C-4E6D-A2AD-1ADAE3B28FA1";
const RUNTIME: &str = "com.apple.CoreSimulator.SimRuntime.iOS-27-0";
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}
fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Value =
        serde_json::from_slice(&fs::read(root().join("examples/minimal.json")).unwrap()).unwrap();
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        config["repository"] = json!(repo);
    }
    fsutil::json(&dir.path().join("app.json"), &config).unwrap();
    let app = App::load(dir.path(), Path::new("app.json")).unwrap();
    (dir, app)
}
fn args(step: &Step) -> Vec<String> {
    step.args
        .iter()
        .map(|a| match a {
            Arg::Text(v) => v.clone(),
            _ => panic!("unexpected secret"),
        })
        .collect()
}
fn test_results(states: &[&str]) -> (Value, Value) {
    let mut summary = json!({"result":if states.contains(&"Failed") {"Failed"} else {"Passed"},"totalTestCount":states.len(),"title":"Suite <&\""});
    for (state, key) in [
        ("Passed", "passedTests"),
        ("Failed", "failedTests"),
        ("Skipped", "skippedTests"),
        ("Expected Failure", "expectedFailures"),
    ] {
        summary[key] = json!(states.iter().filter(|v| **v == state).count());
    }
    let tree = json!({"testNodes":[{"nodeType":"Test Suite","name":"Suite","children":states.iter().enumerate().map(|(i,state)|json!({"nodeType":"Test Case","name":format!("test{i}"),"nodeIdentifier":format!("Suite/test{i}"),"result":state})).collect::<Vec<_>>() }]});
    (summary, tree)
}
fn bootstrap() -> (Value, Value) {
    let read = |name| {
        serde_json::from_slice(
            &fs::read(root().join(format!(
                "scripts/ci/tests/fixtures/simulator-bootstrap-{name}.json"
            )))
            .unwrap(),
        )
        .unwrap()
    };
    (read("summary"), read("tests"))
}

#[test]
fn captured_bootstrap_failure_is_recoverable() {
    let (summary, tree) = bootstrap();
    assert!(results::bootstrap_failure(&summary, &tree));
    assert!(!results::junit(&summary, &tree).unwrap().passed);
}
#[test]
fn assertions_and_executed_tests_are_not_recoverable() {
    let (summary, tree) = bootstrap();
    for message in [
        "XCTAssertTrue failed",
        "Failed to launch application via Xcode",
        "The test runner crashed while running tests",
        "Unknown startup failure",
    ] {
        let mut changed = summary.clone();
        changed["testFailures"][0]["failureText"] = json!(message);
        assert!(!results::bootstrap_failure(&changed, &tree));
    }
    for key in ["passedTests", "skippedTests", "expectedFailures"] {
        let mut changed = summary.clone();
        changed[key] = json!(1);
        assert!(!results::bootstrap_failure(&changed, &tree));
    }
}
#[test]
fn inconsistent_bootstrap_evidence_is_not_recoverable() {
    let (summary, tree) = bootstrap();
    for key in ["totalTestCount", "failedTests"] {
        let mut changed = summary.clone();
        changed[key] = json!(2);
        assert!(!results::bootstrap_failure(&changed, &tree));
    }
    let mut changed = summary.clone();
    changed["passedTests"] = json!(false);
    assert!(!results::bootstrap_failure(&changed, &tree));
    assert!(!results::bootstrap_failure(
        &summary,
        &json!({"testNodes":[null]})
    ));
}
#[test]
fn junit_preserves_failed_skipped_and_expected_cases() {
    let (summary, tree) = test_results(&["Passed", "Failed", "Skipped", "Expected Failure"]);
    let report = results::junit(&summary, &tree).unwrap();
    assert!(!report.passed);
    assert_eq!(report.executed, 4);
    assert_eq!(report.xml.matches("<failure ").count(), 1);
    assert_eq!(report.xml.matches("<skipped ").count(), 2);
    assert!(report.xml.contains("Suite &lt;&amp;&quot;"));
}
#[test]
fn missing_unknown_and_inconsistent_test_results_fail_closed() {
    for states in [vec![], vec!["unknown"]] {
        let (summary, tree) = test_results(&states);
        assert!(results::junit(&summary, &tree).is_err());
    }
    let (mut summary, tree) = test_results(&["Passed"]);
    summary["passedTests"] = json!(2);
    assert!(results::junit(&summary, &tree).is_err());
    let (summary, tree) = test_results(&["Skipped"]);
    assert!(!results::junit(&summary, &tree).unwrap().passed);
}
#[test]
fn explicit_targets_workspace_and_serial_execution_are_preserved() {
    let (_dir, mut app) = fixture();
    app.config.as_object_mut().unwrap().remove("project");
    app.config["workspace"] = json!("Other App.xcworkspace");
    app.config["test_targets"] = json!(["CoreTests", "ExtensionTests"]);
    let command = qa::command(
        &app,
        "test",
        "platform=iOS Simulator,id=exact",
        Path::new("new.xcresult"),
        "1",
    )
    .unwrap();
    let args = args(&command);
    assert_eq!(&args[..2], ["-workspace", "Other App.xcworkspace"]);
    assert!(args.contains(&"-only-testing:ExtensionTests".into()));
    assert!(
        args.windows(2)
            .any(|v| v == ["-parallel-testing-enabled", "NO"])
    );
    assert!(args.contains(&"CODE_SIGNING_ALLOWED=NO".into()));
    assert!(qa::command(&app, "test", "dest", Path::new("bundle"), "3").is_err());
}
#[test]
fn simulator_selection_is_exact_available_and_unique() {
    let inventory =
        json!({"devices":{RUNTIME:[{"name":"iPhone 17","udid":ID,"isAvailable":true}]}});
    assert_eq!(
        toolchain::resolve_inventory(&inventory, RUNTIME, &["iPhone 17".into()]).unwrap()["iPhone 17"],
        ID
    );
    let mut changed = inventory.clone();
    changed["devices"][RUNTIME][0]["isAvailable"] = json!(false);
    assert!(toolchain::resolve_inventory(&changed, RUNTIME, &["iPhone 17".into()]).is_err());
    let mut changed = inventory.clone();
    let device = changed["devices"][RUNTIME][0].clone();
    changed["devices"][RUNTIME]
        .as_array_mut()
        .unwrap()
        .push(device);
    assert!(toolchain::resolve_inventory(&changed, RUNTIME, &["iPhone 17".into()]).is_err());
    assert!(
        toolchain::resolve_inventory(&inventory, "other-runtime", &["iPhone 17".into()]).is_err()
    );
    for bad in ["booted", "all", "C71FB2C5952C4E6DA2AD1ADAE3B28FA1"] {
        assert!(simulator::validate_id(bad).is_err());
    }
}

struct Fake {
    calls: Vec<Step>,
    status: i32,
    boot_status: i32,
    malformed: bool,
}
impl Fake {
    fn new(status: i32) -> Self {
        Self {
            calls: Vec::new(),
            status,
            boot_status: 0,
            malformed: false,
        }
    }
}
impl Executor for Fake {
    fn run(&mut self, step: &Step, _root: &Path, log: Option<&Path>) -> Result<Output> {
        self.calls.push(step.clone());
        if let Some(path) = log {
            fsutil::atomic(path, b"fake tool output\n")?;
        }
        let arguments = args(step);
        let (status, stdout) = match step.program.as_str() {
            "xcodebuild" if arguments == ["-version"] => {
                (0, "Xcode 27.0\nBuild version 27A266a".into())
            }
            "xcodebuild" => (self.status, "".into()),
            "sw_vers" => (0, "26.0".into()),
            "uname" => (0, "arm64".into()),
            "xcrun" if arguments[0] == "--sdk" => (0, "27.0".into()),
            "xcrun" if arguments[0] == "swift" => (0, "Swift 6".into()),
            "xcrun" if arguments.get(1).map(String::as_str) == Some("list") => (
                0,
                json!({"devices":{RUNTIME:[{"name":"iPhone 17","udid":ID,"isAvailable":true}]}})
                    .to_string(),
            ),
            "xcrun" if arguments.get(1).map(String::as_str) == Some("bootstatus") => {
                (self.boot_status, "boot output".into())
            }
            "xcrun" if arguments[0] == "xcresulttool" => {
                if self.malformed {
                    (0, "{}".into())
                } else {
                    let (summary, tree) = test_results(&["Passed"]);
                    (
                        0,
                        if arguments[3] == "summary" {
                            summary.to_string()
                        } else {
                            tree.to_string()
                        },
                    )
                }
            }
            _ => panic!("Unexpected invocation {} {:?}", step.program, arguments),
        };
        Ok(Output { status, stdout })
    }
}
#[test]
fn qa_keeps_xcode_failure_even_when_test_evidence_is_passed() {
    let (_dir, app) = fixture();
    let mut fake = Fake::new(65);
    fsutil::atomic(&app.root.join("qa-results/test/report.junit"), b"<stale/>").unwrap();
    assert_eq!(qa::run(&app, "test", &mut fake).unwrap(), 65);
    assert!(
        !fs::read_to_string(app.root.join("qa-results/test/report.junit"))
            .unwrap()
            .contains("stale")
    );
    let report: Value =
        serde_json::from_slice(&fs::read(app.root.join("qa-results/test/result.json")).unwrap())
            .unwrap();
    assert_eq!(report["status"], 65);
}
#[test]
fn readiness_failure_prevents_xctest_and_removes_stale_junit() {
    let (_dir, app) = fixture();
    let mut fake = Fake::new(0);
    fake.boot_status = 124;
    fsutil::atomic(&app.root.join("qa-results/test/report.junit"), b"<stale/>").unwrap();
    assert_ne!(qa::run(&app, "test", &mut fake).unwrap(), 0);
    assert!(!app.root.join("qa-results/test/report.junit").exists());
    assert!(
        !fake
            .calls
            .iter()
            .any(|s| s.program == "xcodebuild" && args(s).contains(&"test".into()))
    );
    assert_eq!(
        fake.calls
            .iter()
            .filter(|s| args(s).get(1).map(String::as_str) == Some("list"))
            .count(),
        1
    );
}
#[test]
fn malformed_result_cannot_create_green_qa() {
    let (_dir, app) = fixture();
    let mut fake = Fake::new(0);
    fake.malformed = true;
    assert_ne!(qa::run(&app, "test", &mut fake).unwrap(), 0);
    assert!(!app.root.join("qa-results/test/report.junit").exists());
}
#[test]
fn readiness_wait_uses_one_bounded_uuid_command() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::new(0);
    simulator::prepare(
        ID,
        "/configured/Xcode",
        dir.path(),
        &dir.path().join("ready.json"),
        &mut fake,
    )
    .unwrap();
    assert_eq!(fake.calls.len(), 1);
    assert_eq!(args(&fake.calls[0]), ["simctl", "bootstatus", ID, "-b"]);
    assert_eq!(fake.calls[0].timeout_seconds, 180);
    assert!(
        simulator::reset(
            ID,
            "00000000-0000-0000-0000-000000000000",
            "/Xcode",
            dir.path(),
            &mut fake
        )
        .is_err()
    );
    assert_eq!(fake.calls.len(), 1);
}
#[test]
fn xctestrun_environment_is_targeted_in_both_formats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.xctestrun");
    for xml in [
        r#"<plist version="1.0"><dict><key>Tests</key><dict><key>EnvironmentVariables</key><dict><key>KEEP</key><string>yes</string></dict></dict><key>OtherTests</key><dict/></dict></plist>"#,
        r#"<plist version="1.0"><dict><key>TestConfigurations</key><array><dict><key>TestTargets</key><array><dict><key>BlueprintName</key><string>Tests</string></dict></array></dict></array></dict></plist>"#,
    ] {
        fs::write(&path, xml).unwrap();
        screenshots::configure_test_run(&path, "Tests", Path::new("/isolated/host-home")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("<key>SIMULATOR_HOST_HOME</key>").count(), 2);
        assert!(text.contains("/isolated/host-home"));
    }
    assert!(screenshots::configure_test_run(&path, "MissingTarget", Path::new("/unused")).is_err());
}
#[test]
fn screenshot_capture_is_unsigned_and_reuses_a_compiled_test_run() {
    let (_dir, app) = fixture();
    let options = screenshots::Options {
        scheme: "ScreenshotScheme".into(),
        test_target: "UITests".into(),
        devices: vec!["iPhone 18 Pro Max".into()],
        languages: vec!["en-US".into()],
        only_testing: vec![],
        output: "build/capture/images".into(),
        photos: vec![],
        videos: vec![],
    };
    options.validate(&app).unwrap();
    let build = args(&screenshots::build_step(&app, &options, ID, Path::new("products")).unwrap());
    assert!(build.contains(&"build-for-testing".into()));
    assert!(build.contains(&"CODE_SIGNING_ALLOWED=NO".into()));
    let test = args(
        &screenshots::test_step(
            &app,
            &options,
            ID,
            Path::new("same.xctestrun"),
            Path::new("attempt.xcresult"),
        )
        .unwrap(),
    );
    assert_eq!(
        &test[..3],
        ["test-without-building", "-xctestrun", "same.xctestrun"]
    );
    assert!(!test.contains(&"build-for-testing".into()));
    let mut unsafe_options = options.clone();
    unsafe_options.output = "fastlane/screenshots".into();
    assert!(unsafe_options.validate(&app).is_err());
    unsafe_options = options.clone();
    unsafe_options.only_testing = vec!["OtherTests/test()".into()];
    assert!(unsafe_options.validate(&app).is_err());
}
#[test]
fn configured_screenshot_dimensions_are_required() {
    let (_dir, app) = fixture();
    let path = app.root.join("image.png");
    let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    header.extend(1320_u32.to_be_bytes());
    header.extend(2868_u32.to_be_bytes());
    fs::write(&path, &header).unwrap();
    screenshots::validate_png(&app, "iPhone 18 Pro Max", &path).unwrap();
    assert!(screenshots::validate_png(&app, "iPad Pro 13-inch (M5)", &path).is_err());
}
#[test]
fn archive_uses_manual_export_and_preserves_version_contract() {
    let (_dir, app) = fixture();
    let steps = archive::steps(&app, "1.7.0", "77.1").unwrap();
    assert_eq!(steps.len(), 2);
    assert!(args(&steps[0]).contains(&"MARKETING_VERSION=1.7.0".into()));
    assert!(args(&steps[1]).contains(&"-exportArchive".into()));
    let options = archive::options(&app).unwrap();
    let json = serde_json::to_value(options).unwrap();
    assert_eq!(json["signingStyle"], "manual");
    assert_eq!(json["manageAppVersionAndBuildNumber"], false);
    for (version, build) in [
        ("1.7", "77.1"),
        ("1.7.0", "77"),
        ("1.7.0", "0.1"),
        ("1.7.0", "77.0"),
    ] {
        assert!(archive::validate_numbers(version, build).is_err());
    }
    assert!(archive::uuid_set("no symbols").is_err());
    assert_eq!(
        archive::uuid_set(&format!("UUID: {ID} (arm64) App"))
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn paths_reject_traversal_and_config_accepts_independent_consumers() {
    for value in ["../other", "/absolute", "build/../../other", "bad\npath"] {
        assert!(relative_path(value).is_err());
    }
    for value in ["..", "x/y", "x\\y", "bad\n"] {
        assert!(filename(value).is_err());
    }
    let (_dir, app) = fixture();
    assert_eq!(app.text("scheme").unwrap(), "Application");
}
#[cfg(unix)]
#[test]
fn output_symlinks_cannot_escape_the_app() {
    let (_dir, app) = fixture();
    let other = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(other.path(), app.root.join("build")).unwrap();
    assert!(fsutil::confined(&app.root, &app.root.join("build/out/file")).is_err());
    assert!(qa::run(&app, "test", &mut Fake::new(0)).is_err());
}
#[test]
fn format_positions_preserve_reordering_and_reject_mixed_styles() {
    assert_eq!(
        localization::specifiers("%2$@ %1$d %%"),
        Some(vec![(1, "d".into()), (2, "@".into())])
    );
    assert_eq!(localization::specifiers("%@ %1$d"), None);
}
#[test]
fn localization_detects_missing_tokens_and_plural_categories() {
    let catalog = json!({"sourceLanguage":"en","strings":{"Hello ${name}":{"localizations":{"de":{"stringUnit":{"state":"translated","value":"Hallo"}}}}}});
    assert!(
        !localization::catalog_errors(&catalog, &["de".into()])
            .unwrap()
            .is_empty()
    );
    let catalog = json!({"strings":{"Count %d":{"localizations":{"ar":{"variations":{"plural":{"one":{"stringUnit":{"state":"translated","value":"One"}},"other":{"stringUnit":{"state":"translated","value":"%d"}}}}}}}}});
    assert_eq!(
        localization::catalog_errors(&catalog, &["ar".into()])
            .unwrap()
            .len(),
        4
    );
}
#[cfg(unix)]
#[test]
fn timeout_terminates_child_processes_and_pipe_holders() {
    let dir = tempfile::tempdir().unwrap();
    let mut native = Native;
    let started = Instant::now();
    let result = native
        .run(
            &Step::new("/bin/sh", ["-c", "sleep 30 & wait"], 1),
            dir.path(),
            None,
        )
        .unwrap();
    assert_eq!(result.status, 124);
    assert!(started.elapsed() < Duration::from_secs(4));
    let result = native
        .run(
            &Step::new("/bin/sh", ["-c", "sleep 30 & exit 0"], 1),
            dir.path(),
            None,
        )
        .unwrap();
    assert_eq!(result.status, 124);
}
#[test]
fn native_executor_keeps_exit_status_and_inspected_output() {
    let dir = tempfile::tempdir().unwrap();
    let output = Native
        .run(
            &Step::new("/bin/sh", ["-c", "printf 'value'; exit 65"], 5),
            dir.path(),
            None,
        )
        .unwrap();
    assert_eq!(output.status, 65);
    assert_eq!(output.stdout, "value");
}

fn plist_bytes(value: &plist::Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.to_writer_xml(&mut bytes).unwrap();
    bytes
}
fn plist_dict(values: Vec<(&str, plist::Value)>) -> plist::Value {
    let mut dict = plist::Dictionary::new();
    for (key, value) in values {
        dict.insert(key.into(), value);
    }
    dict.into()
}
struct SigningFake {
    entitlements: plist::Value,
    profile: plist::Value,
    wrong_symbols: bool,
    wrong_certificate: bool,
}
impl Executor for SigningFake {
    fn run(&mut self, step: &Step, _root: &Path, _log: Option<&Path>) -> Result<Output> {
        let argv = args(step);
        let stdout = match step.program.as_str() {
            "security" => String::from_utf8(plist_bytes(&self.profile))?,
            "codesign" if argv.contains(&"--entitlements".into()) => {
                String::from_utf8(plist_bytes(&self.entitlements))?
            }
            "codesign" => {
                if let Some(prefix) = argv
                    .iter()
                    .find_map(|value| value.strip_prefix("--extract-certificates="))
                {
                    fs::write(
                        format!("{prefix}0"),
                        if self.wrong_certificate {
                            b"wrong".as_slice()
                        } else {
                            b"leaf".as_slice()
                        },
                    )?;
                }
                String::new()
            }
            "xcrun" => {
                let id = if self.wrong_symbols && !argv.last().unwrap().ends_with(".dSYM") {
                    "11111111-1111-1111-1111-111111111111"
                } else {
                    ID
                };
                format!("UUID: {id} (arm64) executable")
            }
            _ => panic!("Unexpected signing operation"),
        };
        Ok(Output { status: 0, stdout })
    }
}
fn signed_fixture() -> (tempfile::TempDir, App, PathBuf, SigningFake) {
    use std::{io::Write, time::SystemTime};
    let (dir, app) = fixture();
    let info = plist_dict(vec![
        ("CFBundleIdentifier", "org.example.application".into()),
        ("CFBundleShortVersionString", "1.7.0".into()),
        ("CFBundleVersion", "77.1".into()),
        ("DTXcodeBuild", "27A266a".into()),
        ("DTSDKName", "iphoneos27.0".into()),
        ("CFBundleExecutable", "Application".into()),
        ("ITSAppUsesNonExemptEncryption", false.into()),
    ]);
    let privacy = plist_dict(vec![
        ("NSPrivacyTracking", false.into()),
        (
            "NSPrivacyAccessedAPITypes",
            Vec::<plist::Value>::new().into(),
        ),
    ]);
    let entitlements = plist_dict(vec![
        (
            "application-identifier",
            "ABCDE12345.org.example.application".into(),
        ),
        ("com.apple.developer.team-identifier", "ABCDE12345".into()),
        ("get-task-allow", false.into()),
    ]);
    let profile = plist_dict(vec![
        ("UUID", ID.into()),
        (
            "ExpirationDate",
            plist::Date::from(SystemTime::now() + Duration::from_secs(3600)).into(),
        ),
        ("Entitlements", entitlements.clone()),
        (
            "DeveloperCertificates",
            vec![plist::Value::Data(b"leaf".to_vec())].into(),
        ),
    ]);
    fs::create_dir_all(
        app.root
            .join("build/application.xcarchive/dSYMs/Application.app.dSYM"),
    )
    .unwrap();
    let ipa = app.root.join("build/application.ipa");
    let mut zip = zip::ZipWriter::new(fs::File::create(&ipa).unwrap());
    for (name, bytes) in [
        ("Info.plist", plist_bytes(&info)),
        ("PrivacyInfo.xcprivacy", plist_bytes(&privacy)),
        ("Application", b"native executable".to_vec()),
        ("embedded.mobileprovision", b"signed cms".to_vec()),
    ] {
        zip.start_file(
            format!("Payload/Application.app/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
    (
        dir,
        app,
        ipa,
        SigningFake {
            entitlements,
            profile,
            wrong_symbols: false,
            wrong_certificate: false,
        },
    )
}
#[test]
fn exported_archive_requires_authorized_certificate_and_matching_symbols() {
    let (_dir, app, ipa, mut fake) = signed_fixture();
    let inventory = archive::verify(&app, &ipa, "1.7.0", "77.1", &mut fake).unwrap();
    assert_eq!(inventory[0]["bundle_id"], "org.example.application");
    assert!(archive::verify(&app, &ipa, "1.8.0", "77.1", &mut fake).is_err());
    fake.wrong_symbols = true;
    assert!(
        archive::verify(&app, &ipa, "1.7.0", "77.1", &mut fake)
            .unwrap_err()
            .to_string()
            .contains("dSYMs")
    );
    fake.wrong_symbols = false;
    fake.wrong_certificate = true;
    assert!(
        archive::verify(&app, &ipa, "1.7.0", "77.1", &mut fake)
            .unwrap_err()
            .to_string()
            .contains("certificate")
    );
}
#[test]
fn release_export_rejects_debug_profiles_and_malformed_entitlements() {
    let (_dir, app, ipa, mut fake) = signed_fixture();
    fake.entitlements
        .as_dictionary_mut()
        .unwrap()
        .insert("get-task-allow".into(), "true".into());
    assert!(archive::verify(&app, &ipa, "1.7.0", "77.1", &mut fake).is_err());
    fake.entitlements
        .as_dictionary_mut()
        .unwrap()
        .insert("get-task-allow".into(), false.into());
    fake.profile
        .as_dictionary_mut()
        .unwrap()
        .insert("ProvisionedDevices".into(), vec!["device".into()].into());
    assert!(
        archive::verify(&app, &ipa, "1.7.0", "77.1", &mut fake)
            .unwrap_err()
            .to_string()
            .contains("App Store")
    );
}

#[test]
fn public_diagnostics_work_without_app_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let execute = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ios-release"))
            .args(args)
            .current_dir(dir.path())
            .env_remove("IOS_RELEASE_CONFIG")
            .output()
            .unwrap()
    };
    let contract = execute(&["--commands-json"]);
    assert!(contract.status.success());
    let contract: Value = serde_json::from_slice(&contract.stdout).unwrap();
    assert_eq!(contract["schema_version"], 1);
    assert!(contract["commands"]["qa"]["description"].is_string());
    assert_eq!(contract["apple_store_mutations"], false);
    assert!(execute(&["doctor"]).status.success());
    assert!(execute(&["qa", "--help"]).status.success());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}
