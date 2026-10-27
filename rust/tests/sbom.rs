use anyhow::Result;
use ios_release_native::{
    config::App,
    fsutil,
    process::{Arg, Executor, Native, Output, Step},
    sbom, source,
};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

fn document() -> Value {
    json!({"spdxVersion":"SPDX-2.3","dataLicense":"CC0-1.0","SPDXID":"SPDXRef-DOCUMENT",
        "name":"fixture","documentNamespace":"https://example.invalid/fixture",
        "creationInfo":{"created":"2026-01-01T00:00:00Z","creators":["Tool: syft-1.44.0"]},
        "packages":[],"relationships":[]})
}

struct Scanner {
    scans: usize,
    fail_at: Option<usize>,
    malformed: bool,
    version: &'static str,
}
impl Default for Scanner {
    fn default() -> Self {
        Self {
            scans: 0,
            fail_at: None,
            malformed: false,
            version: sbom::SYFT_VERSION,
        }
    }
}
impl Executor for Scanner {
    fn run(&mut self, step: &Step, _: &Path, _: Option<&Path>) -> Result<Output> {
        assert_eq!(step.program, "syft");
        let args: Vec<_> = step
            .args
            .iter()
            .map(|a| match a {
                Arg::Text(v) => v.as_str(),
                _ => panic!(),
            })
            .collect();
        if args[0] == "version" {
            return Ok(Output {
                status: 0,
                stdout: json!({"version":self.version}).to_string(),
            });
        }
        self.scans += 1;
        assert!(args.contains(&"--config"));
        Ok(Output {
            status: if self.fail_at == Some(self.scans) {
                1
            } else {
                0
            },
            stdout: if self.malformed {
                "{}".into()
            } else {
                document().to_string()
            },
        })
    }
}

fn fixture() -> (tempfile::TempDir, App) {
    let temp = tempfile::tempdir().unwrap();
    let config = json!({"repository":"northcutted/picstrip","app_store":{"bundle_id":"dev.example.App"},
        "targets":[{"bundle_id":"dev.example.App"},{"bundle_id":"dev.example.App.Share"}],
        "runtime_dependencies":[{"name":"StaticLibrary","version":"1.2.3","source":"https://example.invalid/static"}]});
    let app = App {
        root: temp.path().to_owned(),
        config,
        config_path: temp.path().join("config.json"),
    };
    fsutil::json(&app.config_path, &app.config).unwrap();
    fs::write(app.root.join("requirements.txt"), "requests==2.32.3\n").unwrap();
    let payload = tempfile::tempdir().unwrap();
    let mut applications = vec![];
    for (folder, id, executable) in [
        ("Payload/App.app", "dev.example.App", "App"),
        (
            "Payload/App.app/PlugIns/Share.appex",
            "dev.example.App.Share",
            "Share",
        ),
    ] {
        let path = payload.path().join(folder);
        fs::create_dir_all(&path).unwrap();
        let info: plist::Value = serde_json::from_value(json!({"CFBundleIdentifier":id,"CFBundleExecutable":executable,"CFBundleShortVersionString":"1.2.3","CFBundleVersion":"7.1"})).unwrap();
        info.to_file_xml(path.join("Info.plist")).unwrap();
        fs::write(
            path.join(executable),
            format!("fixture binary {executable}"),
        )
        .unwrap();
        applications.push(json!({"bundle_id":id,"version":"1.2.3","build_number":"7.1",
            "profile_uuid":"11111111-1111-1111-1111-111111111111","profile_sha256":"a".repeat(64),
            "certificate_sha256":"b".repeat(64),"executable_sha256":fsutil::sha256(&path.join(executable)).unwrap(),"binary_uuids":["fixture-uuid"]}));
    }
    for name in [
        "Payload/App.app/Frameworks/Shared.framework/Shared",
        "Payload/App.app/PlugIns/Share.appex/Frameworks/libExample.dylib",
    ] {
        fsutil::atomic(&payload.path().join(name), b"embedded fixture library").unwrap();
    }
    fs::create_dir_all(app.root.join("build")).unwrap();
    let ipa = app.root.join("build/application.ipa");
    let mut zip = zip::ZipWriter::new(fs::File::create(&ipa).unwrap());
    let mut pending = vec![payload.path().to_owned()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                zip.start_file(
                    path.strip_prefix(payload.path()).unwrap().to_str().unwrap(),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(&fs::read(path).unwrap()).unwrap();
            }
        }
    }
    zip.finish().unwrap();
    fsutil::json(&app.root.join("build/rust-archive.json"), &json!({"applications":applications,
        "version":"1.2.3","build_number":"7.1","source_sha":"c".repeat(40),"run_id":"42",
        "inputs_sha256":source::fingerprint(&app.root).unwrap(),"ipa_sha256":fsutil::sha256(&ipa).unwrap()})).unwrap();
    (temp, app)
}

#[test]
fn source_and_runtime_inventory_are_separate_and_bound_to_the_exact_archive() {
    let (_temp, app) = fixture();
    let report = sbom::generate(&app, &mut Scanner::default()).unwrap();
    assert_eq!(
        report["inventory"]["applications"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        report["inventory"]["embedded_components"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let runtime: Value =
        serde_json::from_slice(&fs::read(app.root.join("build/sbom/ipa.spdx.json")).unwrap())
            .unwrap();
    assert_eq!(runtime["packages"].as_array().unwrap().len(), 5);
    assert_eq!(runtime["packages"][0]["name"], "dev.example.App");
    assert_eq!(runtime["packages"][4]["name"], "StaticLibrary");
    assert!(
        runtime["packages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["licenseConcluded"] == "NOASSERTION")
    );
    sbom::verify(
        &app.root.join("build/sbom"),
        &app.root.join("build/rust-archive.json"),
        "northcutted/picstrip",
    )
    .unwrap();
    let target = app.root.join("build/sealed");
    fs::create_dir(&target).unwrap();
    sbom::attach(&app, &target).unwrap();
    assert_eq!(fs::read_dir(target).unwrap().count(), 3);
}

#[test]
fn changed_source_ipa_archive_and_sbom_are_rejected() {
    for changed in ["source", "ipa", "archive", "sbom", "repository"] {
        let (_temp, app) = fixture();
        sbom::generate(&app, &mut Scanner::default()).unwrap();
        match changed {
            "source" => fs::write(app.root.join("requirements.txt"), "changed").unwrap(),
            "ipa" => fs::write(app.root.join("build/application.ipa"), "changed").unwrap(),
            "archive" => fs::write(app.root.join("build/rust-archive.json"), "{}").unwrap(),
            "sbom" => fs::write(
                app.root.join("build/sbom/ipa.spdx.json"),
                document().to_string(),
            )
            .unwrap(),
            _ => {}
        }
        if matches!(changed, "source" | "ipa") {
            assert!(sbom::generate(&app, &mut Scanner::default()).is_err());
            assert!(!app.root.join("build/sbom/sbom.json").exists());
        } else {
            assert!(
                sbom::verify(
                    &app.root.join("build/sbom"),
                    &app.root.join("build/rust-archive.json"),
                    if changed == "repository" {
                        "other/app"
                    } else {
                        "northcutted/picstrip"
                    }
                )
                .is_err()
            );
        }
    }
}

#[test]
fn interrupted_invalid_or_unreviewed_scans_cannot_leave_stale_success() {
    for scanner in [
        Scanner {
            fail_at: Some(1),
            ..Scanner::default()
        },
        Scanner {
            fail_at: Some(2),
            ..Scanner::default()
        },
        Scanner {
            malformed: true,
            ..Scanner::default()
        },
        Scanner {
            version: "1.55.0",
            ..Scanner::default()
        },
    ] {
        let (_temp, app) = fixture();
        sbom::generate(&app, &mut Scanner::default()).unwrap();
        assert!(sbom::generate(&app, &mut { scanner }).is_err());
        assert!(!app.root.join("build/sbom").exists());
    }
}

#[test]
fn runtime_inventory_rejects_missing_or_mismatched_verified_bundles_and_dependencies() {
    let (_temp, mut app) = fixture();
    let root = tempfile::tempdir().unwrap();
    sbom::extract(&app.root.join("build/application.ipa"), root.path()).unwrap();
    let original: Value =
        serde_json::from_slice(&fs::read(app.root.join("build/rust-archive.json")).unwrap())
            .unwrap();
    for field in ["executable_sha256", "version", "build_number", "bundle_id"] {
        let mut archive = original.clone();
        archive["applications"][0][field] = json!("d".repeat(64));
        assert!(sbom::inventory(&app, root.path(), &archive).is_err());
    }
    app.config["runtime_dependencies"] = json!([{"name":"unknown"}]);
    assert!(sbom::inventory(&app, root.path(), &original).is_err());
    app.config["runtime_dependencies"] = json!([]);
    fs::remove_dir_all(root.path().join("Payload/App.app/PlugIns")).unwrap();
    assert!(sbom::inventory(&app, root.path(), &original).is_err());
}

#[test]
fn unsafe_ipa_paths_and_links_are_rejected_before_extraction() {
    for name in [
        "../outside",
        "/absolute",
        "Payload//double",
        "Payload/./dot",
        "Payload\\backslash",
        "symlink",
    ] {
        let root = tempfile::tempdir().unwrap();
        let zip_path = root.path().join("bad.ipa");
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        if name == "symlink" {
            zip.add_symlink(name, "../outside", zip::write::SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"unsafe").unwrap();
        }
        zip.finish().unwrap();
        let target = root.path().join("out");
        fs::create_dir(&target).unwrap();
        assert!(sbom::extract(&zip_path, &target).is_err(), "{name}");
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    }
}

#[test]
fn augmentation_matches_deployed_python_for_bundles_embedded_and_declared_dependencies() {
    let (_temp, app) = fixture();
    let report = sbom::generate(&app, &mut Scanner::default()).unwrap();
    let platform = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for embedded in [true, false] {
        let mut inventory = report["inventory"].clone();
        if !embedded {
            inventory["embedded_components"] = json!([]);
        }
        let inventory_path = app.root.join("build/inventory.json");
        fsutil::json(&inventory_path, &inventory).unwrap();
        let expected = app.root.join("build/python.spdx.json");
        let result = std::process::Command::new("python3")
            .arg(platform.join("scripts/ci/runtime_sbom.py"))
            .arg(&inventory_path)
            .arg(app.root.join("build/sbom/ipa-syft.spdx.json"))
            .arg(&expected)
            .env(
                "IOS_RELEASE_CONFIG",
                platform.join("examples/picstrip.json"),
            )
            .env_remove("GITHUB_REPOSITORY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let expected: Value = serde_json::from_slice(&fs::read(expected).unwrap()).unwrap();
        assert_eq!(
            sbom::runtime_document("northcutted/picstrip", &inventory, document()).unwrap(),
            expected
        );
    }
}

#[test]
#[ignore = "requires independently installed reviewed Syft 1.44.0"]
fn syft_scans_real_source_and_ipa() {
    let (_temp, app) = fixture();
    sbom::generate(&app, &mut Native).unwrap();
    let read = |name| -> Value {
        serde_json::from_slice(&fs::read(app.root.join("build/sbom").join(name)).unwrap()).unwrap()
    };
    assert!(
        read("source.spdx.json")["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "requests")
    );
    assert!(
        !read("ipa.spdx.json")["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "requests")
    );
    sbom::verify(
        &app.root.join("build/sbom"),
        &app.root.join("build/rust-archive.json"),
        "northcutted/picstrip",
    )
    .unwrap();
}
