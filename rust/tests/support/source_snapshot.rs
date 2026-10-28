use super::*;
use ios_release_native::source_snapshot::{self, FILES};
use std::io::{Read, Write};

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn committed(app: &App) -> String {
    git(&app.root, &["init", "--quiet"]);
    git(
        &app.root,
        &["add", "--", ".ios-release.json", "App.swift", "run.sh"],
    );
    git(
        &app.root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    git(&app.root, &["rev-parse", "HEAD"])
}

fn fixture() -> (tempfile::TempDir, App, String) {
    let (root, app) = app();
    fsutil::atomic(&app.root.join("App.swift"), b"committed app source").unwrap();
    fsutil::atomic(&app.root.join("run.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(app.root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let head = committed(&app);
    (root, app, head)
}

fn archive(report: &Value) -> Value {
    json!({"source_sha":report["source_sha"],"run_id":report["run_id"],"inputs_sha256":report["inputs_sha256"]})
}

#[test]
fn source_snapshot_retains_materialized_inputs_modes_and_excludes_vaults() {
    let (_root, app, head) = fixture();
    // Signing changes and hydrated assets are the bytes compiled, not the original Git blob.
    fsutil::atomic(&app.root.join("App.swift"), b"materialized build input").unwrap();
    fsutil::atomic(
        &app.root.join(".ios-release/signing.vault"),
        b"private encrypted vault",
    )
    .unwrap();
    fsutil::atomic(&app.root.join("build/signing.json"), b"private transfer").unwrap();
    let before_cache = source::fingerprint(&app.root).unwrap();
    fsutil::atomic(
        &app.root.join("scripts/__pycache__/legacy.cpython.pyc"),
        b"generated cache",
    )
    .unwrap();
    assert_eq!(source::fingerprint(&app.root).unwrap(), before_cache);
    let report = source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    let directory = app.root.join("build/source-snapshot");
    assert_eq!(report["files"].as_object().unwrap().len(), 3);
    assert_eq!(report["files"]["run.sh"]["mode"], 0o755);
    assert_eq!(
        report["inputs_sha256"],
        source::fingerprint(&app.root).unwrap()
    );
    assert_eq!(
        source_snapshot::verify(&directory, &archive(&report), &report["run_attempt"]).unwrap(),
        report
    );
    let mut zip = zip::ZipArchive::new(fs::File::open(directory.join(FILES[0])).unwrap()).unwrap();
    let mut data = String::new();
    zip.by_name("App.swift")
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    assert_eq!(data, "materialized build input");
    assert!(
        zip.file_names()
            .all(|p| !p.contains("vault") && !p.starts_with("build/"))
    );
    // Fixed ZIP metadata and sorted files make repeated capture deterministic.
    let repeated = source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    assert_eq!(repeated, report);
    source_snapshot::check_build(&app, &archive(&report), true).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(app.root.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(source_snapshot::check_build(&app, &archive(&report), true).is_err());
}

#[test]
fn source_snapshot_archives_hydrated_lfs_bytes_instead_of_committed_pointers() {
    let (_root, app, _) = fixture();
    fsutil::atomic(
        &app.root.join("asset.bin"),
        b"version https://git-lfs.github.com/spec/v1\noid sha256:fixture\nsize 15\n",
    )
    .unwrap();
    git(&app.root, &["add", "asset.bin"]);
    git(
        &app.root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "asset pointer",
        ],
    );
    let head = git(&app.root, &["rev-parse", "HEAD"]);
    assert!(source_snapshot::capture_for_source(&app, &mut Native, &head).is_err());
    fsutil::atomic(&app.root.join("asset.bin"), b"real asset data").unwrap();
    let report = source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    let folder = app.root.join("build/source-snapshot");
    source_snapshot::verify(&folder, &archive(&report), &report["run_attempt"]).unwrap();
    let mut zip = zip::ZipArchive::new(fs::File::open(folder.join(FILES[0])).unwrap()).unwrap();
    let mut bytes = vec![];
    zip.by_name("asset.bin")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"real asset data");
}

#[test]
fn source_snapshot_failures_remove_stale_evidence() {
    for failure in [
        "head",
        "missing-sha",
        "untracked",
        "deleted",
        "lfs",
        "symlink",
        "submodule",
        "vault",
        "mode",
    ] {
        let (_root, mut app, head) = fixture();
        source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
        let selected = if failure == "missing-sha" {
            String::new()
        } else if failure == "head" {
            "0".repeat(40)
        } else {
            head.clone()
        };
        match failure {
            "untracked" => {
                fsutil::atomic(&app.root.join("secret.env"), b"must not publish").unwrap()
            }
            "deleted" => fs::remove_file(app.root.join("App.swift")).unwrap(),
            "lfs" => fsutil::atomic(
                &app.root.join("App.swift"),
                b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 2\n",
            )
            .unwrap(),
            "symlink" => {
                fs::remove_file(app.root.join("App.swift")).unwrap();
                std::os::unix::fs::symlink("run.sh", app.root.join("App.swift")).unwrap();
            }
            "submodule" => {
                git(
                    &app.root,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("160000,{head},vendor"),
                    ],
                );
                git(
                    &app.root,
                    &[
                        "-c",
                        "commit.gpgsign=false",
                        "commit",
                        "--quiet",
                        "-m",
                        "submodule",
                    ],
                );
            }
            "vault" => {
                app.config["signing"]["vault"] = json!("App.swift");
                app.save().unwrap();
            }
            "mode" => {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(app.root.join("run.sh"), fs::Permissions::from_mode(0o644))
                    .unwrap();
            }
            _ => (),
        }
        let selected = if failure == "submodule" {
            git(&app.root, &["rev-parse", "HEAD"])
        } else {
            selected
        };
        assert!(
            source_snapshot::capture_for_source(&app, &mut Native, &selected).is_err(),
            "{failure}"
        );
        assert!(
            !app.root.join("build/source-snapshot").exists(),
            "{failure}"
        );
    }
}

#[test]
fn source_snapshot_rejects_tampering_and_cross_run_evidence() {
    let (_root, app, head) = fixture();
    let original = source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    let directory = app.root.join("build/source-snapshot");
    for field in [
        "source_sha",
        "run_id",
        "inputs_sha256",
        "run_attempt",
        "archive_sha256",
    ] {
        let mut changed = original.clone();
        changed[field] = json!("wrong");
        fsutil::json(&directory.join(FILES[1]), &changed).unwrap();
        assert!(
            source_snapshot::verify(&directory, &archive(&original), &original["run_attempt"])
                .is_err(),
            "{field}"
        );
    }
    // Refreshing the outer archive hash cannot hide substituted bytes or unsafe members.
    for defect in [
        "bytes",
        "mode",
        "traversal",
        "duplicate",
        "missing",
        "symlink",
        "oversize",
    ] {
        let mut changed = original.clone();
        let mut writer = zip::ZipWriter::new(fs::File::create(directory.join(FILES[0])).unwrap());
        for (name, record) in original["files"].as_object().unwrap() {
            if defect == "missing" && name == "App.swift" {
                continue;
            }
            let entry = if defect == "traversal" && name == "App.swift" {
                "../App.swift"
            } else {
                name
            };
            let mode = if defect == "mode" && name == "run.sh" {
                0o644
            } else {
                record["mode"].as_u64().unwrap() as u32
            };
            let options = zip::write::SimpleFileOptions::default().unix_permissions(mode);
            if defect == "symlink" && name == "App.swift" {
                writer.add_symlink(entry, "run.sh", options).unwrap();
                continue;
            }
            writer.start_file(entry, options).unwrap();
            let bytes = if defect == "bytes" && name == "App.swift" {
                b"changed".to_vec()
            } else {
                fs::read(app.root.join(name)).unwrap()
            };
            writer.write_all(&bytes).unwrap();
            if defect == "duplicate" && name == "App.swift" {
                // zip's writer disallows duplicates, so duplicate the inventory name using a noncanonical alias.
                writer.start_file("./App.swift", options).unwrap();
                writer.write_all(b"alias").unwrap();
            }
        }
        writer.finish().unwrap();
        changed["archive_sha256"] = json!(fsutil::sha256(&directory.join(FILES[0])).unwrap());
        if defect == "oversize" {
            changed["files"]["App.swift"]["size"] = json!(u64::MAX);
        }
        fsutil::json(&directory.join(FILES[1]), &changed).unwrap();
        assert!(
            source_snapshot::verify(&directory, &archive(&original), &original["run_attempt"])
                .is_err(),
            "{defect}"
        );
    }
}

#[test]
fn source_snapshot_handles_an_app_below_the_git_root() {
    let (root, app, _) = fixture();
    let nested = app.root.join("apps/Example");
    fs::create_dir_all(&nested).unwrap();
    fs::rename(
        app.root.join(".ios-release.json"),
        nested.join(".ios-release.json"),
    )
    .unwrap();
    fs::rename(app.root.join("App.swift"), nested.join("App.swift")).unwrap();
    git(root.path(), &["add", "--all"]);
    git(
        root.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "nested",
        ],
    );
    let head = git(root.path(), &["rev-parse", "HEAD"]);
    let nested_app = App::load(&nested, Path::new(".ios-release.json")).unwrap();
    let report = source_snapshot::capture_for_source(&nested_app, &mut Native, &head).unwrap();
    assert_eq!(report["files"].as_object().unwrap().len(), 2);
    assert!(report["files"].get("App.swift").is_some());
}

#[test]
fn source_snapshot_matches_the_existing_component_sorted_fingerprint() {
    let (_root, app, _) = fixture();
    fsutil::atomic(&app.root.join("Example/App.swift"), b"app").unwrap();
    fsutil::atomic(
        &app.root.join("Example.xcodeproj/project.pbxproj"),
        b"project",
    )
    .unwrap();
    git(&app.root, &["add", "Example", "Example.xcodeproj"]);
    git(
        &app.root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "project paths",
        ],
    );
    let head = git(&app.root, &["rev-parse", "HEAD"]);
    let report = source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    assert_eq!(
        report["inputs_sha256"],
        source::fingerprint(&app.root).unwrap()
    );
    source_snapshot::check_build(&app, &archive(&report), true).unwrap();
}

#[test]
fn source_snapshot_is_required_by_sealing_and_survives_package_restore() {
    let (_root, app, head) = fixture();
    let original = candidate(&app);
    let missing = app.root.join("build/missing-source");
    assert!(release::seal(&app, "1.2.3", "7", &missing, false, true).is_err());
    assert!(!missing.exists());
    source_snapshot::capture_for_source(&app, &mut Native, &head).unwrap();
    // Candidate fixtures use the test process's source; bind the fake QA/archive to this real fixture commit.
    for relative in ["build/rust-archive.json", "qa-results/test/result.json"] {
        let path = app.root.join(relative);
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["source_sha"] = json!(head);
        fsutil::json(&path, &value).unwrap();
    }
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ios-release"))
            .current_dir(&app.root)
            .env("SOURCE_SHA", &head)
            .env("IOS_APP_ROOT", &app.root)
            .env("IOS_RELEASE_CONFIG", ".ios-release.json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&[
        "release",
        "seal",
        "--version",
        "1.2.3",
        "--build-number",
        "7",
        "--require-source",
        "--output",
        "build/with-source",
    ]);
    let sealed = app.root.join("build/with-source");
    let restored = store::Release::load(&app, &sealed).unwrap();
    assert_eq!(
        restored.manifest["ipa_sha256"],
        original.manifest["ipa_sha256"]
    );
    let package = app.root.join("build/with-source.zip");
    release::pack(&app, &sealed, &package).unwrap();
    let unpacked = app.root.join("build/restored-source");
    ios_release_native::content::unpack(&app, &package, &unpacked).unwrap();
    assert_eq!(
        store::Release::load(&app, &unpacked).unwrap().manifest,
        restored.manifest
    );
    let mut partial = restored.manifest.clone();
    partial["files"].as_object_mut().unwrap().remove(FILES[0]);
    assert!(ios_release_native::content::names(&partial).is_err());
    fsutil::atomic(&unpacked.join(FILES[0]), b"changed").unwrap();
    assert!(store::Release::load(&app, &unpacked).is_err());
}
