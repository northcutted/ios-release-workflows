use anyhow::{Result, bail};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ios_release_native::{
    api::{self, Api},
    archive,
    config::App,
    fsutil, images, metadata, onboarding,
    process::{Arg, Executor, Native, Step},
    project, release, signing, source, store, vault,
};
use p256::{
    ecdsa::{Signature, SigningKey, signature::Verifier},
    pkcs8::EncodePrivateKey,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
mod support;

const PASSWORD: &str = "separate signing test password";

#[test]
fn nonexistent_output_parents_cannot_hide_traversal_outside_the_app() {
    let (_root, app) = app();
    assert!(fsutil::confined(&app.root, &app.root.join("build/new/../../../outside")).is_err());
    assert!(!app.root.join("build/new").exists());
}

#[test]
fn signing_transfer_rejects_wrong_source_paths_hashes_and_partial_updates() {
    use ios_release_native::signing_inputs::{apply_for_source, export_for_source};
    let (_root, app) = app();
    let project = app.root.join("OrbitNotes.xcodeproj/project.pbxproj");
    fsutil::atomic(&project, b"original project").unwrap();
    fsutil::atomic(&signing::path(&app).unwrap(), b"encrypted vault fixture").unwrap();
    let transfer = app.root.join("build/signing.json");
    let sha = "a".repeat(40);
    export_for_source(&app, &transfer, &sha).unwrap();
    let original: Value = serde_json::from_slice(&fs::read(&transfer).unwrap()).unwrap();
    apply_for_source(&app, &transfer, &sha).unwrap();
    assert!(apply_for_source(&app, &transfer, &"b".repeat(40)).is_err());
    for key in ["path", "sha256", "content"] {
        let mut changed = original.clone();
        let last = changed["files"].as_array().unwrap().len() - 1;
        changed["files"][last][key] = json!(match key {
            "path" => "../outside.p8",
            "sha256" => "wrong checksum",
            _ => "dGFtcGVyZWQ=",
        });
        fsutil::json(&transfer, &changed).unwrap();
        let before = fs::read(&app.config_path).unwrap();
        assert!(apply_for_source(&app, &transfer, &sha).is_err());
        assert_eq!(fs::read(&app.config_path).unwrap(), before);
        assert_eq!(fs::read(&project).unwrap(), b"original project");
    }
}
#[test]
fn authenticated_run_and_invocation_must_match_the_selected_preparation() {
    use ios_release_native::github::{validate_invocation, validate_run};
    let value = json!({"id":42,"run_attempt":1,"repository":{"full_name":"Owner/App"},"conclusion":"success","head_branch":"main","event":"workflow_dispatch","head_sha":"a".repeat(40)});
    assert_eq!(
        validate_run(&value, "owner/app", 42).unwrap(),
        "a".repeat(40)
    );
    assert!(validate_run(&value, "owner/other", 42).is_err());
    assert!(validate_run(&value, "owner/app", 43).is_err());
    for (key, invalid) in [
        ("conclusion", "failure"),
        ("event", "pull_request"),
        ("head_branch", "feature"),
        ("head_sha", "main"),
    ] {
        let mut wrong = value.clone();
        wrong[key] = json!(invalid);
        assert!(validate_run(&wrong, "owner/app", 42).is_err());
    }
    let proof = json!([{"verificationResult":{"signature":{"certificate":{"runInvocationURI":"https://github.com/Owner/App/actions/runs/42/attempts/1"}},"statement":{"predicate":{"runDetails":{"metadata":{"invocationId":"https://github.com/Owner/App/actions/runs/42/attempts/1"}}}}}}]);
    assert!(validate_invocation(&proof, "owner/app", 42, 1).is_ok());
    assert!(validate_invocation(&proof, "owner/app", 42, 2).is_err());
    assert!(validate_invocation(&proof, "owner/app", 42, 0).is_err());
    assert!(validate_invocation(&proof, "owner/app", 43, 1).is_err());
    assert!(validate_invocation(&proof, "owner/other", 42, 1).is_err());
    assert!(validate_invocation(&json!([]), "owner/app", 42, 1).is_err());
    let mut forged = proof.clone();
    forged[0]["verificationResult"]["signature"]["certificate"]["runInvocationURI"] =
        json!("https://github.com/Owner/App/actions/runs/43/attempts/1");
    assert!(validate_invocation(&forged, "owner/app", 42, 1).is_err());
    forged[0]["verificationResult"]["signature"] = json!({});
    assert!(validate_invocation(&forged, "owner/app", 42, 1).is_err());
}
#[test]
fn selected_preparation_attempt_is_exported_without_accepting_output_injection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("github-output");
    fs::write(&path, "existing=value\n").unwrap();
    ios_release_native::github::selection_output(&json!({"run_attempt":"2"}), &path).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(before, b"existing=value\npreparation_attempt=2\n");
    for invalid in [json!(null), json!("0"), json!("2\nother=secret")] {
        assert!(
            ios_release_native::github::selection_output(&json!({"run_attempt":invalid}), &path)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
fn local_metadata(app: &App, locale: &str) {
    let folder = app.path("metadata_path").unwrap().join(locale);
    for (file, value) in [
        ("name.txt", "Orbit Notes"),
        ("description.txt", "Keep your notes on your device."),
        ("support_url.txt", "https://example.com/support"),
        ("privacy_url.txt", "https://example.com/privacy"),
    ] {
        fsutil::atomic(&folder.join(file), value.as_bytes()).unwrap();
    }
}
#[test]
fn metadata_preflight_rejects_placeholders_bad_urls_and_missing_locales() {
    let (_root, app) = app();
    local_metadata(&app, "en-US");
    assert!(metadata::preflight(&app).is_ok());
    let folder = app.path("metadata_path").unwrap().join("en-US");
    for (file, value) in [
        ("description.txt", "Describe what your app does."),
        ("support_url.txt", "not a URL"),
        (
            "privacy_url.txt",
            "https://password:secret@example.com/privacy",
        ),
    ] {
        fsutil::atomic(&folder.join(file), value.as_bytes()).unwrap();
        assert!(metadata::preflight(&app).is_err());
        local_metadata(&app, "en-US");
    }
    local_metadata(&app, "de-DE");
    fs::remove_file(
        app.path("metadata_path")
            .unwrap()
            .join("de-DE/privacy_url.txt"),
    )
    .unwrap();
    assert!(metadata::preflight(&app).is_err());
    let mut candidate = candidate(&app);
    let mut server = AppleFixture::default();
    assert!(metadata::sync(&app, &mut candidate, &mut server).is_err());
    assert!(server.calls.is_empty());
}
#[test]
fn sealed_release_requires_a_complete_verified_bundle_inventory() {
    let (_root, app) = app();
    let release = candidate(&app);
    let mut archive: Value =
        serde_json::from_slice(&fs::read(release.directory.join("archive.json")).unwrap()).unwrap();
    assert!(release::validate_inventory(&app, &archive).is_ok());
    archive["applications"] = json!([]);
    assert!(release::validate_inventory(&app, &archive).is_err());
}
#[test]
fn release_package_contains_only_the_five_verified_public_inputs() {
    let (_root, app) = app();
    let release = candidate(&app);
    fsutil::atomic(&release.directory.join("private.p8"), b"private key").unwrap();
    let package = app.root.join("build/native-release.zip");
    release::pack(&app, &release.directory, &package).unwrap();
    let zip = zip::ZipArchive::new(fs::File::open(package).unwrap()).unwrap();
    assert_eq!(zip.len(), 5);
    assert!(
        !zip.file_names()
            .any(|n| n == "operation.json" || n == "private.p8")
    );
}
#[test]
fn generated_workflow_supports_an_app_inside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    onboarding::write_workflow_for(dir.path(), &"a".repeat(40), "apps/OrbitNotes", "macos-26")
        .unwrap();
    let content = fs::read_to_string(dir.path().join(".github/workflows/ios-native.yml")).unwrap();
    assert!(content.contains("app_root: \"apps/OrbitNotes\""));
    assert!(content.contains("runner: \"macos-26\""));
    assert!(
        onboarding::write_workflow_for(dir.path(), &"a".repeat(40), "../other", "self-hosted")
            .is_err()
    );
}
#[test]
fn child_receives_private_stdin_without_including_it_in_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("secret");
    fsutil::atomic(&input, b"private stdin fixture").unwrap();
    let mut step = Step::new("/bin/cat", std::iter::empty::<String>(), 2);
    step.stdin_file = Some(input);
    assert!(step.args.is_empty());
    assert_eq!(
        Native.run(&step, dir.path(), None).unwrap().stdout,
        "private stdin fixture"
    );
}
fn config() -> Value {
    json!({"schema_version":2,"project":"OrbitNotes.xcodeproj","scheme":"OrbitNotes","team_id":"ABCDE12345","test_targets":["OrbitNotesTests"],"test_device":"iPhone 18 Pro Max","xcode":{"path":"/Applications/Xcode.app/Contents/Developer","version":"27.0","build":"18A123","sdk":"27.0","runtime":"27.0"},"configurations":{"test":"Debug","archive":"AppStore"},"qa_checks":["test"],"targets":[{"name":"OrbitNotes","bundle_id":"dev.example.OrbitNotes","profile":"Orbit App Store","tracking":false,"non_exempt_encryption":false,"entitlements":{}}],"app_store":{"bundle_id":"dev.example.OrbitNotes","release_type":"MANUAL","testflight_groups":["beta"]}})
}
fn app() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    fsutil::json(&dir.path().join(".ios-release.json"), &config()).unwrap();
    let app = App::load(dir.path(), Path::new(".github/ios-release.json")).unwrap();
    (dir, app)
}
fn tests() -> (Value, Value) {
    (
        json!({"result":"Passed","totalTestCount":1,"passedTests":1,"failedTests":0,"skippedTests":0,"expectedFailures":0}),
        json!({"testNodes":[{"nodeType":"Test Case","name":"testNotes","nodeIdentifier":"Notes/testNotes","result":"Passed"}]}),
    )
}
fn qa(app: &App) {
    let (summary, tests) = tests();
    let folder = app.root.join("qa-results/test");
    fsutil::json(&folder.join("summary.json"), &summary).unwrap();
    fsutil::json(&folder.join("tests.json"), &tests).unwrap();
    fsutil::json(&folder.join("result.json"),&json!({"check":"test","status":0,"inputs_sha256":source::fingerprint(&app.root).unwrap(),"source_sha":std::env::var("SOURCE_SHA").or_else(|_|std::env::var("GITHUB_SHA")).ok(),"run_id":std::env::var("GITHUB_RUN_ID").ok()})).unwrap();
}
fn candidate(app: &App) -> store::Release {
    qa(app);
    fsutil::atomic(
        &app.root.join("build/application.ipa"),
        b"fixture IPA bytes",
    )
    .unwrap();
    fsutil::json(&app.root.join("build/rust-archive.json"),&json!({"version":"1.2.3","build_number":"7","inputs_sha256":source::fingerprint(&app.root).unwrap(),"source_sha":std::env::var("SOURCE_SHA").or_else(|_|std::env::var("GITHUB_SHA")).ok(),"run_id":std::env::var("GITHUB_RUN_ID").ok(),"ipa_sha256":fsutil::sha256(&app.root.join("build/application.ipa")).unwrap(),"applications":[{"bundle_id":app.config["app_store"]["bundle_id"],"version":"1.2.3","build_number":"7","profile_uuid":"11111111-1111-1111-1111-111111111111","profile_sha256":"a".repeat(64),"certificate_sha256":"b".repeat(64),"executable_sha256":"c".repeat(64),"binary_uuids":["UUID (arm64)"]}]})).unwrap();
    release::seal(app, "1.2.3", "7", &app.root.join("build/native-release")).unwrap();
    store::Release::load(app, &app.root.join("build/native-release")).unwrap()
}

#[test]
fn vault_roundtrip_and_randomization() {
    let data = json!({"key":"private identity"});
    let a = vault::encrypt(&data, PASSWORD).unwrap();
    let b = vault::encrypt(&data, PASSWORD).unwrap();
    assert_ne!(a, b);
    assert_eq!(vault::decrypt(&a, PASSWORD).unwrap(), data);
    assert!(!a.to_string().contains("private identity"));
}
#[test]
fn vault_rejects_wrong_password_and_modified_bytes() {
    let mut v = vault::encrypt(&json!({"key":"secret"}), PASSWORD).unwrap();
    assert!(vault::decrypt(&v, "another signing password").is_err());
    let mut bytes = STANDARD.decode(v["ciphertext"].as_str().unwrap()).unwrap();
    bytes[0] ^= 1;
    v["ciphertext"] = json!(STANDARD.encode(bytes));
    assert!(vault::decrypt(&v, PASSWORD).is_err());
}
#[test]
fn vault_rejects_short_password_and_unbounded_parameters() {
    assert!(vault::encrypt(&json!({}), "short").is_err());
    let mut v = vault::encrypt(&json!({}), PASSWORD).unwrap();
    v["kdf"] = json!("scrypt-30-8-1");
    assert!(vault::decrypt(&v, PASSWORD).is_err());
}
#[test]
fn vault_is_scoped_to_team_and_all_targets() {
    let (_root, app) = app();
    let mut v = json!({"schema_version":1,"team_id":"ABCDE12345","bundle_ids":["dev.example.OrbitNotes"],"private_key_pem":"PRIVATE KEY"});
    assert!(signing::validate_vault(&app, &v).is_ok());
    v["team_id"] = json!("OTHER12345");
    assert!(signing::validate_vault(&app, &v).is_err());
    v["team_id"] = json!("ABCDE12345");
    v["bundle_ids"] = json!(["dev.example.OtherApp"]);
    assert!(signing::validate_vault(&app, &v).is_err());
}
#[test]
fn jwt_has_apple_claims_and_raw_es256_signature() {
    let key = SigningKey::random(&mut rand::rngs::OsRng);
    let pem = key.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
    let credentials =
        api::Credentials::from_pem("KEY1234567".into(), "issuer".into(), &pem).unwrap();
    let token = credentials.token(123456).unwrap();
    let pieces = token.split('.').collect::<Vec<_>>();
    let header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(pieces[0]).unwrap()).unwrap();
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(pieces[1]).unwrap()).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(claims["aud"], "appstoreconnect-v1");
    assert_eq!(claims["iat"], 123446);
    assert_eq!(claims["exp"], 124056);
    let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(pieces[2]).unwrap()).unwrap();
    key.verifying_key()
        .verify(format!("{}.{}", pieces[0], pieces[1]).as_bytes(), &sig)
        .unwrap();
}
#[test]
fn jwt_rejects_rsa_keys_and_unsafe_key_id() {
    assert!(api::Credentials::from_pem("../key".into(), "issuer".into(), "bad").is_err());
    assert!(
        api::Credentials::from_pem(
            "KEY".into(),
            "issuer".into(),
            "-----BEGIN RSA PRIVATE KEY-----"
        )
        .is_err()
    );
}
fn operations() -> Value {
    json!([{"method":"PUT","url":"https://upload.apple.com/asset?capability=secret","offset":0,"length":3,"requestHeaders":[]},{"method":"PUT","url":"https://upload.apple.com/asset2","offset":3,"length":2,"requestHeaders":[]}])
}
#[test]
fn upload_ranges_must_cover_exact_bytes() {
    assert!(api::validate_operations(5, &operations()).is_ok());
    for value in [json!(6), json!(3), json!(0)] {
        let mut ops = operations();
        ops[1]["length"] = value;
        assert!(api::validate_operations(5, &ops).is_err());
    }
    let mut ops = operations();
    ops[1]["offset"] = json!(2);
    assert!(api::validate_operations(5, &ops).is_err());
}
#[test]
fn uploads_never_forward_credentials_or_follow_plaintext_urls() {
    for header in ["Authorization", "Cookie", "Host", "Content-Length"] {
        let mut ops = operations();
        ops[0]["requestHeaders"] = json!([{"name":header,"value":"secret"}]);
        assert!(api::validate_operations(5, &ops).is_err());
    }
    for url in [
        "http://upload.apple.com/a",
        "https://user:password@upload.apple.com/a",
        "https://upload.apple.com:8080/a",
    ] {
        let mut ops = operations();
        ops[0]["url"] = json!(url);
        assert!(api::validate_operations(5, &ops).is_err());
    }
}
#[test]
fn v2_initialization_allows_qa_before_signing() {
    let dir = tempfile::tempdir().unwrap();
    fsutil::json(
        &dir.path().join(".ios-release.json"),
        &json!({"schema_version":2,"project":"Example.xcodeproj","scheme":"Example"}),
    )
    .unwrap();
    let app = App::load(dir.path(), Path::new(".github/ios-release.json")).unwrap();
    assert!(app.require_archive().is_err());
    assert_eq!(app.configuration("archive"), "Release");
}
#[test]
fn compliance_cannot_be_guessed() {
    let (_root, mut app) = app();
    app.config["targets"][0]["tracking"] = Value::Null;
    assert!(app.require_archive().is_err());
    app.config["targets"][0]["tracking"] = json!(false);
    app.config["targets"][0]["non_exempt_encryption"] = Value::Null;
    assert!(app.require_archive().is_err());
}
#[test]
fn apple_integer_build_numbers_and_custom_configurations() {
    let (_root, app) = app();
    let steps = archive::steps(&app, "1.2.3", "7").unwrap();
    let args = steps[0]
        .args
        .iter()
        .filter_map(|v| {
            if let Arg::Text(s) = v {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(args.windows(2).any(|p| p == ["-configuration", "AppStore"]));
    assert!(archive::steps(&app, "1.2.3", "7.2.1").is_ok());
    assert!(archive::steps(&app, "1.2.3", "7;echo secret").is_err());
}
#[test]
fn qa_without_named_tests_is_rejected() {
    let (_root, mut app) = app();
    app.config["test_targets"] = json!([]);
    assert!(
        ios_release_native::qa::command(&app, "test", "destination", Path::new("out"), "1")
            .is_err()
    );
}
#[test]
fn stale_qa_does_not_seal_a_changed_source() {
    let (_root, app) = app();
    qa(&app);
    assert!(release::verify_qa(&app).is_ok());
    fsutil::atomic(&app.root.join("App.swift"), b"modified source").unwrap();
    assert!(release::verify_qa(&app).is_err());
}
#[test]
fn release_rejects_changed_ipa_and_qa() {
    for file in ["application.ipa", "qa.json", "archive.json", "app.json"] {
        let (_root, app) = app();
        let release = candidate(&app);
        fsutil::atomic(&release.directory.join(file), b"changed").unwrap();
        assert!(store::Release::load(&app, &release.directory).is_err());
    }
}
#[test]
fn receipts_are_bound_to_one_app_version_and_ipa() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    release
        .checkpoint(json!({"identity":{"bundle_id":"wrong"}}))
        .unwrap();
    assert!(store::Release::load(&app, &release.directory).is_err());
}
#[test]
fn sealed_releases_cannot_be_overwritten() {
    let (_root, app) = app();
    let release = candidate(&app);
    assert!(release::seal(&app, "1.2.3", "7", &release.directory).is_err());
}
#[test]
fn project_changes_only_archive_app_and_extension_signing() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/OrbitNotes/OrbitNotes.xcodeproj/project.pbxproj"),
    )
    .unwrap();
    let targets = vec![
        json!({"name":"OrbitNotes","profile":"App Profile"}),
        json!({"name":"OrbitShare","profile":"Share Profile"}),
    ];
    let changed = project::signed_project(&source, &targets, "AppStore", "ABCDE12345").unwrap();
    assert_eq!(changed.matches("CODE_SIGN_STYLE = \"Manual\"").count(), 2);
    assert!(changed.contains("App Profile"));
    assert!(changed.contains("Share Profile"));
    assert_eq!(
        project::signed_project(&changed, &targets, "AppStore", "ABCDE12345").unwrap(),
        changed
    );
    assert!(project::signed_project(&source, &targets, "Missing", "ABCDE12345").is_err());
}
#[test]
fn project_parser_rejects_duplicate_and_unterminated_values() {
    for source in ["{a=1;a=2;}", "{a=\"unterminated;}", "{/*unfinished"] {
        assert!(project::parse(source).is_err());
    }
}
#[test]
fn discovers_test_only_targets_and_custom_archive_configuration() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/OrbitNotes")
        .canonicalize()
        .unwrap();
    let settings = json!([{"target":"OrbitNotes","buildSettings":{"PRODUCT_TYPE":"com.apple.product-type.application","PRODUCT_BUNDLE_IDENTIFIER":"dev.fixture.Orbit","PROJECT_FILE_PATH":fixture.join("OrbitNotes.xcodeproj")}}]);
    let devices = json!({"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-27-0":[{"name":"iPhone 18 Pro Max","isAvailable":true}]}});
    let config = onboarding::config_from_settings(
        &fixture,
        "project",
        "OrbitNotes.xcodeproj",
        "OrbitNotes",
        json!({"runtime":"27.0"}),
        &devices,
        &settings,
        &onboarding::Options::default(),
    )
    .unwrap();
    assert_eq!(config["test_targets"], json!(["OrbitNotesTests"]));
    assert_eq!(config["configurations"]["archive"], "AppStore");
    assert!(config["targets"][0]["tracking"].is_null());
    assert!(config["targets"][0]["non_exempt_encryption"].is_null());
}
#[test]
fn generated_actions_are_pinned_and_do_not_overwrite_existing_work() {
    let dir = tempfile::tempdir().unwrap();
    assert!(onboarding::write_workflow(dir.path(), "main").is_err());
    onboarding::write_workflow(dir.path(), &"a".repeat(40)).unwrap();
    assert!(onboarding::write_workflow(dir.path(), &"b".repeat(40)).is_err());
    let content = fs::read_to_string(dir.path().join(".github/workflows/ios-native.yml")).unwrap();
    assert!(content.contains(&format!("native-app.yml@{}", "a".repeat(40))));
    assert!(content.contains("'pull_request' && 'check'"));
}
#[test]
fn app_store_screenshot_classes_match_current_api_names() {
    assert_eq!(metadata::display_type(1320, 2868).unwrap(), "APP_IPHONE_67");
    assert_eq!(metadata::display_type(2868, 1320).unwrap(), "APP_IPHONE_67");
    assert_eq!(metadata::display_type(1242, 2208).unwrap(), "APP_IPHONE_55");
    assert_eq!(
        metadata::display_type(2064, 2752).unwrap(),
        "APP_IPAD_PRO_3GEN_129"
    );
    assert!(metadata::display_type(200, 300).is_err());
}
#[test]
fn screenshots_are_normalized_without_alpha() {
    let mut original = vec![];
    {
        let mut e = png::Encoder::new(&mut original, 1, 1);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 128])
            .unwrap();
    }
    let result = images::store_png(&original).unwrap();
    assert_eq!(result[25], 2);
    let mut r = png::Decoder::new(std::io::Cursor::new(result))
        .read_info()
        .unwrap();
    let mut rgb = [0; 3];
    r.next_frame(&mut rgb).unwrap();
    assert_eq!(rgb, [255, 127, 127]);
    assert!(images::store_png(b"not PNG").is_err());
}
#[test]
fn step_local_secrets_are_used_without_global_environment_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let mut step = Step::new("/bin/echo", std::iter::empty::<String>(), 2);
    step.args = vec![Arg::Secret {
        secret_env: "IOS_RELEASE_TEST_LOCAL_SECRET".into(),
    }];
    step.env.insert(
        "IOS_RELEASE_TEST_LOCAL_SECRET".into(),
        "private-local-value".into(),
    );
    assert_eq!(
        Native.run(&step, dir.path(), None).unwrap().stdout.trim(),
        "private-local-value"
    );
    assert!(std::env::var("IOS_RELEASE_TEST_LOCAL_SECRET").is_err());
}

#[derive(Default)]
struct MetadataFixture {
    resources: BTreeMap<String, Value>,
    posts: usize,
    patches: usize,
    mismatch: bool,
}
impl Api for MetadataFixture {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        support::validate(method, path, query, body)?;
        if path == "/v1/apps" {
            return Ok(
                json!({"data":[{"id":"app","attributes":{"bundleId":"dev.example.OrbitNotes"}}]}),
            );
        }
        if path == "/v1/apps/app/appInfos" {
            return Ok(
                json!({"data":[{"id":"info","attributes":{"state":"PREPARE_FOR_SUBMISSION"}}]}),
            );
        }
        if method == "POST" {
            self.posts += 1;
            let mut value = body.unwrap()["data"].clone();
            value["id"] = json!(format!("resource-{}", self.posts));
            let route = format!(
                "/v1/{}/{}",
                value["type"].as_str().unwrap(),
                value["id"].as_str().unwrap()
            );
            self.resources.insert(route, value.clone());
            return Ok(json!({"data":value}));
        }
        if method == "PATCH" {
            self.patches += 1;
            let resource = self.resources.get_mut(path).unwrap();
            for (key, value) in body.unwrap()["data"]["attributes"].as_object().unwrap() {
                resource["attributes"][key] = value.clone();
            }
            return Ok(json!({"data":resource}));
        }
        if let Some(resource) = self.resources.get(path) {
            let mut resource = resource.clone();
            if self.mismatch {
                resource["attributes"]["name"] = json!("Changed by another actor");
            }
            return Ok(json!({"data":resource}));
        }
        if path.ends_with("appStoreReviewDetail") {
            return Ok(
                json!({"data":self.resources.values().find(|v|v["type"]=="appStoreReviewDetails")}),
            );
        }
        for kind in [
            "appStoreVersionLocalizations",
            "appInfoLocalizations",
            "betaGroups",
        ] {
            if path.ends_with(kind) {
                return Ok(
                    json!({"data":self.resources.values().filter(|v|v["type"]==kind).collect::<Vec<_>>()}),
                );
            }
        }
        bail!("Unexpected metadata fixture endpoint {method} {path}")
    }
    fn transfer(&mut self, _: &Path, _: &Value) -> Result<()> {
        bail!("No screenshot assets in this fixture")
    }
}
#[test]
fn locale_and_review_metadata_use_current_apple_contracts_and_readback() {
    let (_root, app) = app();
    local_metadata(&app, "en-US");
    fsutil::json(&app.root.join("store/review.json"), &json!({"contactFirstName":"App","contactLastName":"Owner","contactPhone":"+1 555 0100","contactEmail":"owner@example.com","demoAccountRequired":false,"notes":"All features work offline."})).unwrap();
    let mut candidate = candidate(&app);
    candidate
        .checkpoint(json!({"version_id":"version"}))
        .unwrap();
    let mut server = MetadataFixture::default();
    metadata::sync(&app, &mut candidate, &mut server).unwrap();
    assert_eq!(server.posts, 3);
    metadata::sync(&app, &mut candidate, &mut server).unwrap();
    assert_eq!(server.posts, 3);
    assert_eq!(server.patches, 3);
    server.mismatch = true;
    assert!(metadata::sync(&app, &mut candidate, &mut server).is_err());
}
#[test]
fn beta_group_setup_reuses_the_app_group_and_saves_its_id() {
    let (_root, mut app) = app();
    app.config["app_store"]["testflight_groups"] = json!([]);
    let mut server = MetadataFixture::default();
    let created = store::beta_groups(&mut app, &mut server, Some("Team"), false).unwrap();
    assert_eq!(
        app.config["app_store"]["testflight_groups"],
        json!([created["id"]])
    );
    store::beta_groups(&mut app, &mut server, Some("Team"), false).unwrap();
    assert_eq!(server.posts, 1);
    assert!(store::beta_groups(&mut app, &mut server, Some("Team"), true).is_err());
    let list = store::beta_groups(&mut app, &mut server, None, false).unwrap();
    assert_eq!(list["groups"][0]["name"], "Team");
}

#[derive(Default)]
struct AppleFixture {
    upload: Option<Value>,
    file: Option<Value>,
    build: bool,
    version: Option<Value>,
    selected: Value,
    review: Option<Value>,
    items: Vec<Value>,
    group: bool,
    foreign_group: bool,
    lost: Option<String>,
    calls: BTreeMap<String, usize>,
    wrong_marketing: bool,
}
impl AppleFixture {
    fn done() -> Self {
        Self {
            upload: Some(
                json!({"type":"buildUploads","id":"upload","attributes":{"cfBundleShortVersionString":"1.2.3","cfBundleVersion":"7","platform":"IOS","state":{"state":"COMPLETE"}}}),
            ),
            ..Default::default()
        }
    }
    fn build(&self) -> Value {
        json!({"type":"builds","id":"build","attributes":{"version":"7","processingState":"VALID","usesNonExemptEncryption":false},"relationships":{"app":{"data":{"id":"app"}},"preReleaseVersion":{"data":{"id":"pre"}}}})
    }
    fn count(&self, method: &str, path: &str) -> usize {
        *self.calls.get(&format!("{method} {path}")).unwrap_or(&0)
    }
    fn complete(&mut self, release: &store::Release) {
        self.upload = Self::done().upload;
        self.file = Some(
            json!({"type":"buildUploadFiles","id":"asset","attributes":{"fileSize":17,"assetDeliveryState":{"state":"COMPLETE"},"sourceFileChecksums":{"file":{"algorithm":"SHA_256","hash":release.manifest["ipa_sha256"]}}}}),
        );
        self.build = true;
    }
}
impl Api for AppleFixture {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        data: Option<&Value>,
    ) -> Result<Value> {
        support::validate(method, path, query, data)?;
        let key = format!("{method} {path}");
        *self.calls.entry(key.clone()).or_default() += 1;
        let body = data.cloned().unwrap_or(Value::Null);
        let answer = match (method, path) {
            ("GET", "/v1/apps") => {
                json!({"data":[{"id":"app","attributes":{"bundleId":"dev.example.OrbitNotes"}}]})
            }
            ("GET", "/v1/builds") => json!({"data":[]}),
            ("GET", "/v1/apps/app/buildUploads") => {
                json!({"data":self.upload.iter().collect::<Vec<_>>()})
            }
            ("POST", "/v1/buildUploads") => {
                assert_eq!(body["data"]["attributes"]["platform"], "IOS");
                self.upload = Self::done().upload;
                self.upload.as_mut().unwrap()["attributes"]["state"]["state"] =
                    json!("AWAITING_UPLOAD");
                json!({"data":self.upload})
            }
            ("GET", "/v1/buildUploads/upload") => {
                let mut upload = self.upload.clone().unwrap();
                upload["relationships"]["assetFile"]["data"] = if self.file.is_some() {
                    json!({"id":"asset"})
                } else {
                    Value::Null
                };
                upload["relationships"]["build"]["data"] = if self.build {
                    json!({"id":"build"})
                } else {
                    Value::Null
                };
                json!({"data":upload,"included":self.file.iter().collect::<Vec<_>>()})
            }
            ("POST", "/v1/buildUploadFiles") => {
                assert_eq!(body["data"]["attributes"]["uti"], "com.apple.ipa");
                self.file = Some(
                    json!({"id":"asset","type":"buildUploadFiles","attributes":{"fileSize":17,"assetDeliveryState":{"state":"AWAITING_UPLOAD"},"uploadOperations":[]}}),
                );
                json!({"data":self.file})
            }
            ("PATCH", "/v1/buildUploadFiles/asset") => {
                self.file.as_mut().unwrap()["attributes"]["sourceFileChecksums"] =
                    body["data"]["attributes"]["sourceFileChecksums"].clone();
                self.file.as_mut().unwrap()["attributes"]["assetDeliveryState"]["state"] =
                    json!("COMPLETE");
                self.upload.as_mut().unwrap()["attributes"]["state"]["state"] = json!("COMPLETE");
                self.build = true;
                json!({"data":self.file})
            }
            ("GET", "/v1/builds/build") => {
                json!({"data":self.build(),"included":[{"id":"pre","type":"preReleaseVersions","attributes":{"version":if self.wrong_marketing{"9.9.9"}else{"1.2.3"},"platform":"IOS"}}]})
            }
            ("GET", "/v1/apps/app/appStoreVersions") => {
                json!({"data":self.version.iter().collect::<Vec<_>>()})
            }
            ("POST", "/v1/appStoreVersions") => {
                let mut v = body["data"].clone();
                v["id"] = json!("version");
                v["attributes"]["appVersionState"] = json!("PREPARE_FOR_SUBMISSION");
                self.version = Some(v);
                json!({"data":self.version})
            }
            ("GET", "/v1/appStoreVersions/version/build") => json!({"data":self.selected}),
            ("PATCH", "/v1/appStoreVersions/version/relationships/build") => {
                self.selected = body["data"].clone();
                json!({})
            }
            ("PATCH", "/v1/appStoreVersions/version") => {
                for (k, v) in body["data"]["attributes"].as_object().unwrap() {
                    self.version.as_mut().unwrap()["attributes"][k] = v.clone();
                }
                json!({"data":self.version})
            }
            ("GET", "/v1/appStoreVersions/version") => json!({"data":self.version}),
            ("GET", "/v1/apps/app/reviewSubmissions") => {
                json!({"data":self.review.iter().collect::<Vec<_>>()})
            }
            ("POST", "/v1/reviewSubmissions") => {
                self.review =
                    Some(json!({"id":"review","attributes":{"state":"READY_FOR_REVIEW"}}));
                json!({"data":self.review})
            }
            ("GET", "/v1/reviewSubmissions/review/items") => json!({"data":self.items}),
            ("POST", "/v1/reviewSubmissionItems") => {
                self.items.push(body["data"].clone());
                json!({"data":{"id":"item"}})
            }
            ("GET", "/v1/reviewSubmissions/review") => json!({"data":self.review}),
            ("PATCH", "/v1/reviewSubmissions/review") => {
                self.review.as_mut().unwrap()["attributes"]["state"] = json!("WAITING_FOR_REVIEW");
                self.version.as_mut().unwrap()["attributes"]["appVersionState"] =
                    json!("WAITING_FOR_REVIEW");
                json!({"data":self.review})
            }
            ("GET", "/v1/betaGroups/beta") => {
                json!({"data":{"id":"beta","attributes":{"isInternalGroup":true},"relationships":{"app":{"data":{"id":if self.foreign_group{"another-app"}else{"app"}}}}}})
            }
            ("GET", "/v1/betaGroups/beta/builds") => {
                assert!(query.iter().all(|(k, _)| k == "limit"));
                json!({"data":if self.group{vec![self.build()]}else{vec![]}})
            }
            ("POST", "/v1/betaGroups/beta/relationships/builds") => {
                self.group = true;
                json!({})
            }
            _ => bail!("Unexpected fixture request {key}"),
        };
        if self.lost.as_deref() == Some(&key) {
            self.lost = None;
            bail!("Simulated lost mutation response");
        }
        Ok(answer)
    }
    fn transfer(&mut self, file: &Path, _: &Value) -> Result<()> {
        assert_eq!(fs::read(file)?, b"fixture IPA bytes");
        Ok(())
    }
}
#[test]
fn prepared_release_keeps_its_reviewed_release_and_encryption_policy() {
    let (_root, mut app) = app();
    let mut release = candidate(&app);
    app.config["app_store"]["release_type"] = json!("AFTER_APPROVAL");
    app.config["targets"][0]["non_exempt_encryption"] = json!(true);
    let mut server = AppleFixture::done();
    server.complete(&release);
    store::stage(&app, &mut release, &mut server).unwrap();
    assert_eq!(
        server.version.unwrap()["attributes"]["releaseType"],
        "MANUAL"
    );
}
#[test]
fn native_upload_stage_and_review_are_idempotent() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    store::upload(&app, &mut release, &mut api).unwrap();
    store::upload(&app, &mut release, &mut api).unwrap();
    store::stage(&app, &mut release, &mut api).unwrap();
    store::submit(&app, &mut release, &mut api, true).unwrap();
    store::submit(&app, &mut release, &mut api, true).unwrap();
    assert_eq!(api.count("POST", "/v1/buildUploads"), 1);
    assert_eq!(api.count("POST", "/v1/buildUploadFiles"), 1);
    assert_eq!(api.count("POST", "/v1/reviewSubmissions"), 1);
    assert_eq!(api.count("POST", "/v1/reviewSubmissionItems"), 1);
    assert_eq!(api.count("PATCH", "/v1/reviewSubmissions/review"), 1);
}
#[test]
fn interrupted_upload_reservation_is_reconciled_without_duplicate_post() {
    for lost in [
        "POST /v1/buildUploads",
        "POST /v1/buildUploadFiles",
        "PATCH /v1/buildUploadFiles/asset",
    ] {
        let (_root, app) = app();
        let mut release = candidate(&app);
        let mut api = AppleFixture {
            lost: Some(lost.into()),
            ..Default::default()
        };
        assert!(store::upload(&app, &mut release, &mut api).is_err());
        let mut release = store::Release::load(&app, &release.directory).unwrap();
        store::upload(&app, &mut release, &mut api).unwrap();
        assert_eq!(api.count("POST", "/v1/buildUploads"), 1);
        assert_eq!(api.count("POST", "/v1/buildUploadFiles"), 1);
        assert!(
            store::processed(&app, &mut release, &mut api)
                .unwrap()
                .is_some()
        );
    }
}
#[test]
fn unowned_unfinished_reservations_are_not_overwritten() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::done();
    api.upload.as_mut().unwrap()["attributes"]["state"]["state"] = json!("AWAITING_UPLOAD");
    assert!(store::upload(&app, &mut release, &mut api).is_err());
    assert_eq!(api.count("POST", "/v1/buildUploadFiles"), 0);
}
#[test]
fn wrong_marketing_version_cannot_be_selected() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    api.complete(&release);
    api.wrong_marketing = true;
    assert!(store::stage(&app, &mut release, &mut api).is_err());
    assert_eq!(api.count("POST", "/v1/appStoreVersions"), 0);
}
#[test]
fn stage_never_replaces_a_foreign_selected_build() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    api.complete(&release);
    api.selected = json!({"id":"another-build"});
    assert!(store::stage(&app, &mut release, &mut api).is_err());
    assert_eq!(
        api.count("PATCH", "/v1/appStoreVersions/version/relationships/build"),
        0
    );
}
#[test]
fn submission_requires_confirmation_before_any_request() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    assert!(store::submit(&app, &mut release, &mut api, false).is_err());
    assert!(api.calls.is_empty());
}
#[test]
fn lost_review_response_resumes_its_owned_submission() {
    for lost in [
        "POST /v1/reviewSubmissions",
        "POST /v1/reviewSubmissionItems",
        "PATCH /v1/reviewSubmissions/review",
    ] {
        let (_root, app) = app();
        let mut release = candidate(&app);
        let mut api = AppleFixture::default();
        api.complete(&release);
        store::stage(&app, &mut release, &mut api).unwrap();
        api.lost = Some(lost.into());
        assert!(store::submit(&app, &mut release, &mut api, true).is_err());
        store::submit(&app, &mut release, &mut api, true).unwrap();
        assert_eq!(api.count("POST", "/v1/reviewSubmissions"), 1);
        assert_eq!(api.count("POST", "/v1/reviewSubmissionItems"), 1);
    }
}
#[test]
fn unrelated_active_review_is_not_adopted() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    api.complete(&release);
    store::stage(&app, &mut release, &mut api).unwrap();
    api.review = Some(json!({"id":"review","attributes":{"state":"READY_FOR_REVIEW"}}));
    assert!(store::submit(&app, &mut release, &mut api, true).is_err());
    assert_eq!(api.count("POST", "/v1/reviewSubmissionItems"), 0);
}
#[test]
fn testflight_assignment_is_app_scoped_and_idempotent() {
    let (_root, app) = app();
    let mut release = candidate(&app);
    let mut api = AppleFixture::default();
    api.complete(&release);
    api.foreign_group = true;
    assert!(store::testflight(&app, &mut release, &mut api).is_err());
    assert!(!api.group);
    api.foreign_group = false;
    store::testflight(&app, &mut release, &mut api).unwrap();
    store::testflight(&app, &mut release, &mut api).unwrap();
    assert_eq!(
        api.count("POST", "/v1/betaGroups/beta/relationships/builds"),
        1
    );
}
