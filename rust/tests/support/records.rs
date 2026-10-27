use super::*;
use ios_release_native::{
    github,
    process::Output,
    provenance::{self, Identity},
    records,
};

struct Server {
    producer: Identity,
    github: Value,
    slsa: Value,
    releases: Vec<Value>,
    tags: BTreeMap<String, String>,
    files: BTreeMap<u64, BTreeMap<String, Vec<u8>>>,
    mutations: usize,
    fail_upload: bool,
    fail_proof: bool,
    immutable: bool,
    main_only: bool,
    corrupt_readback: bool,
    actions_downloads: usize,
    actions_preparation: Option<std::path::PathBuf>,
    artifacts: Vec<Value>,
}
impl Server {
    fn attestation(identity: &Identity) -> Value {
        let uri = format!(
            "https://github.com/{}/actions/runs/{}/attempts/{}",
            identity.repository, identity.run, identity.attempt
        );
        json!([{"verificationResult":{"signature":{"certificate":{"runInvocationURI":uri}},"statement":{"predicate":{"runDetails":{"metadata":{"invocationId":uri}}}}}}])
    }
    fn assets(&self, id: u64) -> Value {
        use sha2::{Digest, Sha256};
        json!(self.files.get(&id).into_iter().flat_map(|f| f.iter()).map(|(name, bytes)| {
            json!({"name":name,"state":"uploaded","digest":format!("sha256:{:x}", Sha256::digest(bytes))})
        }).collect::<Vec<_>>())
    }
}
impl Executor for Server {
    fn run(&mut self, step: &Step, _: &Path, _: Option<&Path>) -> Result<Output> {
        let args: Vec<_> = step
            .args
            .iter()
            .map(|a| match a {
                Arg::Text(s) => s.as_str(),
                _ => panic!(),
            })
            .collect();
        let flag = |name| args.iter().position(|a| *a == name).map(|i| args[i + 1]);
        let value = if step.program == "slsa-verifier" {
            if self.fail_proof {
                bail!("invalid signature");
            }
            self.slsa.clone()
        } else if args[..2] == ["attestation", "verify"] {
            assert!(args.contains(&"--bundle"));
            if self.fail_proof {
                bail!("invalid signature");
            }
            if args[2].ends_with("operation.json") {
                let bytes = fs::read(args[2])?;
                let receipt: Value = serde_json::from_slice(&bytes)?;
                let mut identity = self.producer.clone();
                identity.run = receipt["test_operation_run"].as_u64().unwrap_or(50);
                Self::attestation(&identity)
            } else {
                self.github.clone()
            }
        } else if args[..2] == ["release", "download"] {
            let tag = args[2];
            let id = self.releases.iter().find(|r| r["tag_name"] == tag).unwrap()["id"]
                .as_u64()
                .unwrap();
            for (name, bytes) in &self.files[&id] {
                fsutil::atomic(&Path::new(flag("--dir").unwrap()).join(name), bytes)?;
            }
            json!({})
        } else if args[..2] == ["run", "download"] {
            self.actions_downloads += 1;
            if let Some(directory) = &self.actions_preparation {
                for name in ["native-release.zip", provenance::BUNDLE, provenance::PROOF] {
                    fsutil::atomic(
                        &Path::new(flag("--dir").unwrap()).join(name),
                        &fs::read(directory.join(name))?,
                    )?;
                }
                json!({})
            } else {
                bail!("All Actions artifacts have expired");
            }
        } else {
            assert_eq!(args[0], "api", "{args:?}");
            let method = flag("--method").unwrap();
            let endpoint = args
                .iter()
                .find(|a| a.starts_with("repos/") || a.starts_with("https://uploads.github.com/"))
                .unwrap();
            let prefix = format!("repos/{}", self.producer.repository);
            let path = endpoint.strip_prefix(&prefix).unwrap_or(endpoint);
            let body = flag("--input").map(fs::read).transpose()?;
            if method != "GET" {
                self.mutations += 1;
            }
            match (method, path) {
                ("GET", "/immutable-releases") => json!({"enabled":self.immutable}),
                ("GET", "/environments/release-publishing") => {
                    json!({"deployment_branch_policy":{"custom_branch_policies":true,"protected_branches":false}})
                }
                ("GET", "/environments/release-publishing/deployment-branch-policies") => {
                    json!({"branch_policies":[{"name":if self.main_only {"main"} else {"*"},"type":"branch"}]})
                }
                ("GET", "/releases?per_page=100&page=1") => json!(self.releases),
                ("GET", p) if p.starts_with("/git/matching-refs/tags/") => {
                    let tag = p.trim_start_matches("/git/matching-refs/tags/");
                    json!(self.tags.get(tag).map(|sha| vec![json!({"ref":format!("refs/tags/{tag}"),"object":{"type":"commit","sha":sha}})]).unwrap_or_default())
                }
                ("POST", "/git/refs") => {
                    let body: Value = serde_json::from_slice(body.as_ref().unwrap())?;
                    let tag = body["ref"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("refs/tags/")
                        .unwrap();
                    assert!(
                        self.tags
                            .insert(tag.into(), body["sha"].as_str().unwrap().into())
                            .is_none()
                    );
                    body
                }
                ("POST", "/releases") => {
                    let mut value: Value = serde_json::from_slice(body.as_ref().unwrap())?;
                    assert_eq!(value["make_latest"], "false");
                    value["id"] = json!(100 + self.releases.len() as u64);
                    value["html_url"] = json!("https://github.com/owner/app/releases/tag/fixture");
                    self.releases.push(value.clone());
                    value
                }
                ("POST", p) if p.starts_with("https://uploads.github.com/") => {
                    assert!(
                        p.starts_with(&format!("https://uploads.github.com/{prefix}/releases/"))
                    );
                    assert_eq!(
                        flag("--header"),
                        Some("Content-Type: application/octet-stream")
                    );
                    if self.fail_upload && self.files.values().any(|f| !f.is_empty()) {
                        bail!("upload interrupted");
                    }
                    let (operation, name) = p.split_once("/assets?name=").unwrap();
                    let id = operation.rsplit('/').next().unwrap().parse::<u64>()?;
                    assert!(
                        self.files
                            .entry(id)
                            .or_default()
                            .insert(name.into(), body.unwrap())
                            .is_none(),
                        "never overwrite assets"
                    );
                    json!({})
                }
                ("GET", p) if p.contains("/assets?") => {
                    let id = p.split('/').nth(2).unwrap().parse::<u64>()?;
                    let mut value = self.assets(id);
                    if self.corrupt_readback
                        && let Some(first) = value.as_array_mut().unwrap().first_mut()
                    {
                        first["digest"] = json!("sha256:changed");
                    }
                    value
                }
                ("GET" | "PATCH", p) if p.starts_with("/releases/") => {
                    let id = p.trim_start_matches("/releases/").parse::<u64>()?;
                    let value = self.releases.iter_mut().find(|r| r["id"] == id).unwrap();
                    if method == "PATCH" {
                        let patch: Value = serde_json::from_slice(body.as_ref().unwrap())?;
                        assert_eq!(patch, json!({"draft":false,"make_latest":"false"}));
                        value["draft"] = json!(false);
                        value["immutable"] = json!(true);
                    }
                    value.clone()
                }
                ("GET", p) if p.starts_with("/compare/") => json!({"status":"ahead"}),
                ("GET", "/actions/runs/42") => {
                    json!({"id":42,"repository":{"full_name":self.producer.repository},"conclusion":"success","head_branch":"main","event":"workflow_dispatch","run_attempt":2,
                    "head_sha":self.producer.source_sha,"path":self.producer.workflow,"referenced_workflows":[{"path":format!("northcutted/ios-release-workflows/.github/workflows/native-app.yml@{}", env!("IOS_RELEASE_BUILD_REVISION")),"sha":env!("IOS_RELEASE_BUILD_REVISION")}]})
                }
                ("GET", "/actions/artifacts?per_page=100&page=1") => {
                    json!({"artifacts":self.artifacts})
                }
                _ => bail!("unexpected {method} {path}"),
            }
        };
        Ok(Output {
            status: 0,
            stdout: value.to_string(),
        })
    }
}

fn fixture() -> (tempfile::TempDir, App, std::path::PathBuf, Server) {
    let (root, mut app) = app();
    let repository = std::env::var("GITHUB_REPOSITORY").unwrap_or_else(|_| "owner/app".into());
    app.config["repository"] = json!(repository);
    app.save().unwrap();
    let original = candidate(&app);
    let producer = Identity {
        repository,
        source_sha: "a".repeat(40),
        workflow: ".github/workflows/native.yml".into(),
        run: 42,
        attempt: 2,
        event: "workflow_dispatch".into(),
    };
    let mut manifest = original.manifest;
    for (file, hash_key) in [("archive.json", "archive_sha256"), ("qa.json", "qa_sha256")] {
        let path = original.directory.join(file);
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if file == "qa.json" {
            for result in value.as_object_mut().unwrap().values_mut() {
                result["source_sha"] = json!(producer.source_sha);
                result["run_id"] = json!("42");
            }
        } else {
            value["source_sha"] = json!(producer.source_sha);
            value["run_id"] = json!("42");
        }
        fsutil::json(&path, &value).unwrap();
        manifest[hash_key] = json!(fsutil::sha256(&path).unwrap());
        manifest["files"][file] = manifest[hash_key].clone();
    }
    manifest["source_sha"] = json!(producer.source_sha);
    manifest["source_workflow"] = json!(producer.workflow);
    manifest["run_id"] = json!("42");
    manifest["run_attempt"] = json!("2");
    fsutil::json(&original.directory.join("release.json"), &manifest).unwrap();
    let directory = app.root.join("build/native-preparation");
    fs::create_dir(&directory).unwrap();
    let package = directory.join("native-release.zip");
    release::pack(&app, &original.directory, &package).unwrap();
    for name in [provenance::BUNDLE, provenance::PROOF] {
        fs::write(directory.join(name), b"signed fixture").unwrap();
    }
    let hashes = provenance::hashes(&original.directory, &package).unwrap();
    let slsa = json!({"_type":"https://in-toto.io/Statement/v0.1","predicateType":"https://slsa.dev/provenance/v0.2",
        "subject":hashes.iter().map(|(n,h)|json!({"name":n,"digest":{"sha256":h}})).collect::<Vec<_>>(),
        "predicate":{"builder":{"id":provenance::BUILDER},"buildType":"https://github.com/slsa-framework/slsa-github-generator/generic@v1",
            "invocation":{"configSource":{"uri":format!("git+https://github.com/{}@refs/heads/main", producer.repository),"digest":{"sha1":producer.source_sha},"entryPoint":producer.workflow},
                "environment":{"github_run_id":"42","github_run_attempt":"2","github_event_name":"workflow_dispatch","github_ref":"refs/heads/main"}}}});
    let server = Server {
        github: Server::attestation(&producer),
        producer,
        slsa,
        releases: vec![],
        tags: BTreeMap::new(),
        files: BTreeMap::new(),
        mutations: 0,
        fail_upload: false,
        fail_proof: false,
        immutable: true,
        main_only: true,
        corrupt_readback: false,
        actions_downloads: 0,
        actions_preparation: None,
        artifacts: vec![],
    };
    (root, app, directory, server)
}

#[test]
fn actions_only_preparations_remain_usable_without_enabling_durable_publication() {
    let (_root, app, directory, mut server) = fixture();
    server.actions_preparation = Some(directory);
    let output = app.root.join("build/actions-recovered");
    github::fetch(&app, 42, &output, &mut server).unwrap();
    assert_eq!(
        fs::read(output.join("application.ipa")).unwrap(),
        b"fixture IPA bytes"
    );
    assert_eq!(server.actions_downloads, 1);
    assert_eq!(server.mutations, 0);
}

#[test]
fn durable_preparation_resumes_interrupted_publication_and_recovers_after_artifact_expiry() {
    let (_root, app, directory, mut server) = fixture();
    let identity = server.producer.clone();
    server.fail_upload = true;
    assert!(records::record_preparation(&app, &directory, &identity, &mut server).is_err());
    assert_eq!(server.releases.len(), 1);
    assert_eq!(server.releases[0]["draft"], true);
    assert_eq!(server.files[&100].len(), 1);
    server.fail_upload = false;
    records::record_preparation(&app, &directory, &identity, &mut server).unwrap();
    assert_eq!(server.releases[0]["immutable"], true);
    let writes = server.mutations;
    records::record_preparation(&app, &directory, &identity, &mut server).unwrap();
    assert_eq!(server.mutations, writes);
    let output = app.root.join("build/recovered");
    github::fetch(&app, 42, &output, &mut server).unwrap();
    assert_eq!(
        fs::read(output.join("application.ipa")).unwrap(),
        b"fixture IPA bytes"
    );
    assert_eq!(server.actions_downloads, 0);
    assert_eq!(server.releases.len(), 1);
}

#[test]
fn record_publication_rejects_invalid_proofs_controls_and_conflicting_tags_before_mutation() {
    for failure in ["proof", "immutable", "environment", "tag"] {
        let (_root, app, directory, mut server) = fixture();
        let identity = server.producer.clone();
        match failure {
            "proof" => server.fail_proof = true,
            "immutable" => server.immutable = false,
            "environment" => server.main_only = false,
            _ => {
                server
                    .tags
                    .insert("native-preparation-42-2".into(), "b".repeat(40));
            }
        }
        assert!(
            records::record_preparation(&app, &directory, &identity, &mut server).is_err(),
            "{failure}"
        );
        assert_eq!(server.mutations, 0, "{failure}");
    }
}

#[test]
fn record_retry_rejects_conflicting_assets_and_incomplete_readback_without_replacement() {
    for failure in ["conflict", "extra", "digest", "body", "duplicate"] {
        let (_root, app, directory, mut server) = fixture();
        let identity = server.producer.clone();
        server.fail_upload = true;
        assert!(records::record_preparation(&app, &directory, &identity, &mut server).is_err());
        server.fail_upload = false;
        match failure {
            "conflict" => {
                *server
                    .files
                    .get_mut(&100)
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap() = b"foreign bytes".to_vec();
            }
            "extra" => {
                server
                    .files
                    .get_mut(&100)
                    .unwrap()
                    .insert("unexpected.txt".into(), vec![1]);
            }
            "digest" => server.corrupt_readback = true,
            "body" => server.releases[0]["body"] = json!("another handoff"),
            _ => server.releases.push(server.releases[0].clone()),
        }
        let before = server.files.clone();
        let mutations = server.mutations;
        assert!(
            records::record_preparation(&app, &directory, &identity, &mut server).is_err(),
            "{failure}"
        );
        assert_eq!(server.files, before);
        assert_eq!(server.mutations, mutations);
    }
}

#[test]
fn bad_durable_evidence_cannot_fall_back_to_actions_or_expose_a_release() {
    for failure in ["signature", "tag", "mutable", "draft", "bytes", "producer"] {
        let (_root, app, directory, mut server) = fixture();
        let identity = server.producer.clone();
        records::record_preparation(&app, &directory, &identity, &mut server).unwrap();
        match failure {
            "signature" => server.fail_proof = true,
            "tag" => {
                server
                    .tags
                    .insert("native-preparation-42-2".into(), "b".repeat(40));
            }
            "mutable" => server.releases[0]["immutable"] = json!(false),
            "draft" => server.releases[0]["draft"] = json!(true),
            "bytes" => {
                server
                    .files
                    .get_mut(&100)
                    .unwrap()
                    .insert("native-release.zip".into(), b"altered".to_vec());
            }
            _ => {
                let bytes = server
                    .files
                    .get_mut(&100)
                    .unwrap()
                    .get_mut("native-record.json")
                    .unwrap();
                let mut value: Value = serde_json::from_slice(bytes).unwrap();
                value["producer"]["run"] = json!(43);
                *bytes = serde_json::to_vec(&value).unwrap();
            }
        }
        let output = app.root.join("build/recovered");
        assert!(
            github::fetch(&app, 42, &output, &mut server).is_err(),
            "{failure}"
        );
        assert!(!output.exists());
        assert_eq!(server.actions_downloads, 0);
    }
}

#[test]
fn durable_receipt_restores_interrupted_operations_and_rejects_another_ipa() {
    let (_root, app, directory, mut server) = fixture();
    let identity = server.producer.clone();
    records::record_preparation(&app, &directory, &identity, &mut server).unwrap();
    let original = store::Release::load(&app, &app.root.join("build/native-release")).unwrap();
    let receipt_dir = app.root.join("build/native-receipt");
    fs::create_dir(&receipt_dir).unwrap();
    fs::write(
        receipt_dir.join(records::RECEIPT_BUNDLE),
        b"signed operation fixture",
    )
    .unwrap();
    let mut receipt = original.receipt;
    receipt["upload_id"] = json!("interrupted-upload");
    let mut operation = identity.clone();
    operation.run = 50;
    receipt["test_operation_run"] = json!(50);
    fsutil::json(&receipt_dir.join("operation.json"), &receipt).unwrap();
    records::record_receipt(&app, &receipt_dir, 42, 2, &operation, &mut server).unwrap();
    // A late artifact from an older operation must not supersede durable run 50.
    server.artifacts = vec![
        json!({"id":999,"expired":false,"name":"native-receipt-42-2-49-2","workflow_run":{"id":49}}),
    ];
    let output = app.root.join("build/recovered");
    github::fetch(&app, 42, &output, &mut server).unwrap();
    let restored = store::Release::load(&app, &output).unwrap();
    assert_eq!(restored.receipt["upload_id"], "interrupted-upload");
    assert_eq!(server.actions_downloads, 0);
    operation.run = 51;
    receipt["test_operation_run"] = json!(51);
    receipt["identity"]["ipa_sha256"] = json!("other");
    fsutil::json(&receipt_dir.join("operation.json"), &receipt).unwrap();
    let before = server.mutations;
    assert!(records::record_receipt(&app, &receipt_dir, 42, 2, &operation, &mut server).is_err());
    assert_eq!(server.mutations, before);
    server.files.get_mut(&101).unwrap().insert(
        "operation.json".into(),
        serde_json::to_vec(&receipt).unwrap(),
    );
    let output = app.root.join("build/tampered-recovery");
    assert!(github::fetch(&app, 42, &output, &mut server).is_err());
    assert!(!output.exists());
}
