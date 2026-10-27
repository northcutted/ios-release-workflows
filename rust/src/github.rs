//! GitHub integration uses the authenticated official CLI. App code is never executed here.
use crate::{
    config::App,
    fsutil, onboarding,
    process::{Executor, Step, checked},
    signing,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::io::Write;
use std::{fs, path::Path};

pub fn repository(app: &App) -> Result<&str> {
    let repo = app.config["repository"]
        .as_str()
        .context("Set the GitHub repository with init --repository owner/name")?;
    ensure!(
        regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")?.is_match(repo),
        "Invalid GitHub repository"
    );
    Ok(repo)
}
pub fn validate_runner(runner: &str) -> Result<()> {
    ensure!(
        [
            "xcode-27",
            "macos-26",
            "macos-26-intel",
            "macos-15",
            "macos-15-intel"
        ]
        .contains(&runner),
        "Choose a supported hosted macOS runner"
    );
    Ok(())
}
pub fn controls(app: &App, runner: &str, executor: &mut impl Executor) -> Result<()> {
    validate_runner(runner)?;
    ensure!(
        std::env::var("GITHUB_EVENT_NAME").as_deref() == Ok("workflow_dispatch")
            && std::env::var("GITHUB_REF").as_deref() == Ok("refs/heads/main"),
        "Native release operations require an explicit dispatch from main"
    );
    let repo = repository(app)?;
    let info = request(executor, &app.root, "GET", &format!("repos/{repo}"), None)?;
    ensure!(
        info["default_branch"] == "main",
        "Native releases require main as the default branch"
    );
    for environment in [
        "native-signing-admin",
        "native-signing",
        "native-app-store",
        "native-production",
    ] {
        let value = request(
            executor,
            &app.root,
            "GET",
            &format!("repos/{repo}/environments/{environment}"),
            None,
        )?;
        ensure!(
            value["deployment_branch_policy"]["custom_branch_policies"] == true
                && value["deployment_branch_policy"]["protected_branches"] == false,
            "Environment {environment} is not restricted to main; run github setup"
        );
        let policies = request(
            executor,
            &app.root,
            "GET",
            &format!("repos/{repo}/environments/{environment}/deployment-branch-policies"),
            None,
        )?;
        ensure!(
            policies["branch_policies"]
                .as_array()
                .is_some_and(|v| v.len() == 1
                    && v[0]["name"] == "main"
                    && v[0]["type"] == "branch"),
            "Environment {environment} permits another branch/tag; run github setup"
        );
        if environment == "native-production" {
            ensure!(
                value["can_admins_bypass"] == false,
                "Native production permits administrators to bypass approval; run github setup"
            );
            ensure!(
                value["protection_rules"]
                    .as_array()
                    .is_some_and(
                        |rules| rules.iter().any(|r| r["type"] == "required_reviewers"
                            && r["reviewers"].as_array().is_some_and(|r| !r.is_empty()))
                    ),
                "Native production has no human approval gate; run github setup"
            );
        }
    }
    Ok(())
}
pub(crate) fn request(
    executor: &mut impl Executor,
    root: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    ensure!(
        path.starts_with("repos/") || path == "user" || path.starts_with("users/"),
        "Unexpected GitHub API path"
    );
    let temp = tempfile::tempdir()?;
    let input = temp.path().join("request.json");
    let mut args = vec![
        "api".into(),
        "--hostname".into(),
        "github.com".into(),
        "--method".into(),
        method.into(),
        path.into(),
    ];
    if let Some(body) = body {
        fsutil::json(&input, body)?;
        args.extend(["--input".into(), input.to_string_lossy().into_owned()]);
    }
    let raw = checked(executor, &Step::new("gh", args, 120), root)?;
    if raw.is_empty() {
        Ok(json!({}))
    } else {
        Ok(serde_json::from_str(&raw)?)
    }
}
pub fn setup(app: &App, reviewer: Option<&str>, executor: &mut impl Executor) -> Result<()> {
    app.require_signing()?;
    let repo = repository(app)?;
    let account = request(executor, &app.root, "GET", "user", None)?;
    let login = reviewer
        .or_else(|| account["login"].as_str())
        .context("GitHub reviewer missing")?;
    ensure!(
        regex::Regex::new(r"^[A-Za-z0-9-]+$")?.is_match(login),
        "Invalid GitHub reviewer login"
    );
    let owner = request(executor, &app.root, "GET", &format!("users/{login}"), None)?;
    let id = owner["id"].as_u64().context("GitHub reviewer ID missing")?;
    let repo_info = request(executor, &app.root, "GET", &format!("repos/{repo}"), None)?;
    let branch = repo_info["default_branch"]
        .as_str()
        .context("Repository has no default branch")?;
    crate::config::filename(branch)?;
    ensure!(
        branch == "main",
        "Native Actions currently require a protected main default branch"
    );
    let credentials = onboarding::local_credentials()?;
    let key_file = credentials["key_file"]
        .as_str()
        .context("Apple key file missing")?;
    let key = fs::read_to_string(key_file)?;
    crate::api::Credentials::from_pem(
        credentials["key_id"]
            .as_str()
            .context("Key ID missing")?
            .into(),
        credentials["issuer_id"]
            .as_str()
            .context("Issuer ID missing")?
            .into(),
        &key,
    )?;
    let password = signing::password()?;
    ensure!(
        password.len() >= 16,
        "Signing password must contain at least 16 characters"
    );
    signing::validate_vault(
        app,
        &crate::vault::read(&signing::path(app)?, &password)
            .context("Cannot unlock the signing vault; GitHub secrets were not changed")?,
    )?;
    // API secrets and signing keys have distinct scopes. Production always has an explicit owner gate.
    for environment in [
        "native-signing-admin",
        "native-signing",
        "native-app-store",
        "native-production",
    ] {
        let reviewers = if environment == "native-production" {
            json!([{"type":"User","id":id}])
        } else {
            json!([])
        };
        let body = json!({"wait_timer":0,"reviewers":reviewers,"prevent_self_review":false,"can_admins_bypass":false,"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}});
        request(
            executor,
            &app.root,
            "PUT",
            &format!("repos/{repo}/environments/{environment}"),
            Some(&body),
        )?;
        let policies = request(
            executor,
            &app.root,
            "GET",
            &format!("repos/{repo}/environments/{environment}/deployment-branch-policies"),
            None,
        )?;
        let branches = policies["branch_policies"]
            .as_array()
            .context("GitHub environment branch rules missing")?;
        ensure!(
            branches
                .iter()
                .all(|p| p["name"] == branch && p["type"] == "branch"),
            "Environment {environment} has extra deployment rules; remove them before setup"
        );
        if branches.is_empty() {
            request(
                executor,
                &app.root,
                "POST",
                &format!("repos/{repo}/environments/{environment}/deployment-branch-policies"),
                Some(&json!({"name":branch,"type":"branch"})),
            )?;
        }
        let values = if environment == "native-signing" {
            vec![("IOS_RELEASE_SIGNING_PASSWORD", password.clone())]
        } else {
            let mut values = vec![
                (
                    "IOS_RELEASE_API_KEY_ID",
                    credentials["key_id"].as_str().unwrap().into(),
                ),
                (
                    "IOS_RELEASE_API_ISSUER_ID",
                    credentials["issuer_id"].as_str().unwrap().into(),
                ),
                ("IOS_RELEASE_API_KEY_CONTENT", key.clone()),
            ];
            if environment == "native-signing-admin" {
                values.push(("IOS_RELEASE_SIGNING_PASSWORD", password.clone()));
            }
            if ["native-app-store", "native-production"].contains(&environment)
                && let Ok(value) = std::env::var("IOS_RELEASE_DEMO_PASSWORD")
            {
                ensure!(!value.is_empty(), "Demo account password is empty");
                values.push(("IOS_RELEASE_DEMO_PASSWORD", value));
            }
            values
        };
        for (name, value) in values {
            let temporary = tempfile::tempdir()?;
            let secret = temporary.path().join("secret");
            fsutil::atomic(&secret, value.as_bytes())?;
            let mut step = Step::new(
                "gh",
                ["secret", "set", name, "--repo", repo, "--env", environment],
                120,
            );
            step.env.insert("GH_HOST".into(), "github.com".into());
            step.stdin_file = Some(secret);
            checked(executor, &step, &app.root)?;
        }
        let actual = request(
            executor,
            &app.root,
            "GET",
            &format!("repos/{repo}/environments/{environment}"),
            None,
        )?;
        ensure!(
            actual["deployment_branch_policy"] == body["deployment_branch_policy"],
            "Environment branch restriction readback differs"
        );
        if environment == "native-production" {
            ensure!(
                actual["can_admins_bypass"] == false,
                "GitHub did not disable administrator approval bypass"
            );
            ensure!(
                actual["protection_rules"]
                    .as_array()
                    .is_some_and(
                        |rules| rules.iter().any(|r| r["type"] == "required_reviewers"
                            && r["reviewers"]
                                .as_array()
                                .is_some_and(|v| v.iter().any(|r| r["reviewer"]["id"] == id)))
                    ),
                "GitHub did not confirm the production approval gate; check your repository plan"
            );
        }
    }
    println!(
        "Native Actions configured. Only main can use release environments; {login} must approve App Review and public releases. Commit .ios-release.json, its encrypted signing vault, project signing settings and the generated workflow."
    );
    Ok(())
}

pub fn validate_run<'a>(value: &'a Value, repo: &str, run: u64) -> Result<&'a str> {
    ensure!(
        value["id"].as_u64() == Some(run)
            && value["repository"]["full_name"]
                .as_str()
                .is_some_and(|r| r.eq_ignore_ascii_case(repo)),
        "GitHub run belongs to another repository"
    );
    ensure!(
        value["conclusion"] == "success"
            && value["head_branch"] == "main"
            && value["event"] == "workflow_dispatch"
            && value["run_attempt"].as_u64().is_some_and(|n| n > 0),
        "Select a successful preparation from main"
    );
    let sha = value["head_sha"]
        .as_str()
        .context("Run source SHA missing")?;
    ensure!(
        sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
        "Run source SHA invalid"
    );
    Ok(sha)
}

pub(crate) fn verify_attestation(
    app: &App,
    file: &Path,
    bundle: Option<&Path>,
    source: &str,
    run: u64,
    attempt: u64,
    executor: &mut impl Executor,
) -> Result<()> {
    let repo = repository(app)?;
    let mut args: Vec<String> = vec![
        "attestation".into(),
        "verify".into(),
        file.to_str().context("Invalid artifact path")?.into(),
        "--repo".into(),
        repo.into(),
        "--signer-workflow".into(),
        "northcutted/ios-release-workflows/.github/workflows/native-app.yml".into(),
        "--signer-digest".into(),
        env!("IOS_RELEASE_BUILD_REVISION").into(),
        "--source-digest".into(),
        source.into(),
        "--source-ref".into(),
        "refs/heads/main".into(),
        "--deny-self-hosted-runners".into(),
        "--predicate-type".into(),
        "https://slsa.dev/provenance/v1".into(),
        "--format".into(),
        "json".into(),
    ];
    if let Some(bundle) = bundle {
        ensure!(
            bundle.is_file()
                && !bundle.is_symlink()
                && fs::metadata(bundle)?.len() <= 2 * 1024 * 1024,
            "GitHub attestation bundle is missing, unsafe or oversized"
        );
        args.extend([
            "--bundle".into(),
            bundle.to_str().context("Invalid attestation path")?.into(),
        ]);
    }
    let raw = checked(executor, &Step::new("gh", args, 180).github(), &app.root)?;
    let result: Value = serde_json::from_str(&raw)?;
    validate_invocation(&result, repo, run, attempt)
}
pub fn validate_invocation(result: &Value, repo: &str, run: u64, attempt: u64) -> Result<()> {
    ensure!(run > 0 && attempt > 0, "A run and attempt are required");
    let expected = format!("https://github.com/{repo}/actions/runs/{run}/attempts/{attempt}");
    ensure!(
        result
            .as_array()
            .is_some_and(|values| values.iter().any(|v| {
                let proof = &v["verificationResult"];
                // This certificate field comes from GitHub's OIDC token; predicate
                // metadata alone can be supplied by the workflow that signs it.
                proof["signature"]["certificate"]["runInvocationURI"]
                    .as_str()
                    .is_some_and(|id| {
                        id.eq_ignore_ascii_case(&expected)
                        && proof["statement"]["predicate"]["runDetails"]["metadata"]
                            ["invocationId"]
                            .as_str()
                            .is_some_and(|claim| claim.eq_ignore_ascii_case(id))
                    })
            })),
        "Verified attestation does not identify the selected workflow run"
    );
    Ok(())
}
fn download(
    app: &App,
    run: u64,
    name: &str,
    executor: &mut impl Executor,
) -> Result<tempfile::TempDir> {
    let temp = tempfile::tempdir()?;
    checked(
        executor,
        &Step::new(
            "gh",
            [
                "run",
                "download",
                &run.to_string(),
                "--repo",
                repository(app)?,
                "--name",
                name,
                "--dir",
                temp.path().to_str().unwrap(),
            ],
            300,
        )
        .github(),
        &app.root,
    )?;
    Ok(temp)
}
fn selected_run(app: &App, run: u64, executor: &mut impl Executor) -> Result<Value> {
    let repo = repository(app)?;
    let value = request(
        executor,
        &app.root,
        "GET",
        &format!("repos/{repo}/actions/runs/{run}"),
        None,
    )?;
    let sha = validate_run(&value, repo, run)?;
    let producer = format!(
        "northcutted/ios-release-workflows/.github/workflows/native-app.yml@{}",
        env!("IOS_RELEASE_BUILD_REVISION")
    );
    ensure!(
        value["referenced_workflows"].as_array().is_some_and(|v| v
            .iter()
            .any(|r| r["path"] == producer && r["sha"] == env!("IOS_RELEASE_BUILD_REVISION"))),
        "Run did not use this reviewed native release workflow"
    );
    let comparison = request(
        executor,
        &app.root,
        "GET",
        &format!("repos/{repo}/compare/{sha}...main"),
        None,
    )?;
    ensure!(
        ["ahead", "identical"].contains(&comparison["status"].as_str().unwrap_or("")),
        "Selected source is not an ancestor of main"
    );
    Ok(value)
}
pub fn fetch(app: &App, run: u64, output: &Path, executor: &mut impl Executor) -> Result<Value> {
    ensure!(run > 0, "Select a preparation run ID");
    let identity = preparation_identity(app, run, executor)?;
    let attempt = identity.attempt;
    let temp = match crate::records::download_preparation(app, &identity, executor)? {
        Some(directory) => directory,
        None => download(
            app,
            run,
            &format!("native-release-{run}-{attempt}"),
            executor,
        )?,
    };
    fsutil::confined(&app.root, output)?;
    ensure!(
        !output.exists(),
        "Release directory already exists; resume its saved receipt instead"
    );
    let (_stage, mut release) = authenticate_preparation(app, temp.path(), &identity, executor)?;
    restore_receipt(app, run, attempt, &mut release, executor)?;
    let manifest = release.manifest.clone();
    fs::create_dir_all(output.parent().context("Output needs a parent")?)?;
    fs::rename(&release.directory, output)?;
    println!("Authenticated preparation {run} at {}", output.display());
    Ok(manifest)
}

pub(crate) fn preparation_identity(
    app: &App,
    run: u64,
    executor: &mut impl Executor,
) -> Result<crate::provenance::Identity> {
    let selected = selected_run(app, run, executor)?;
    let source = selected["head_sha"].as_str().unwrap();
    let attempt = selected["run_attempt"].as_u64().unwrap();
    Ok(crate::provenance::Identity {
        repository: repository(app)?.into(),
        source_sha: source.into(),
        workflow: selected["path"]
            .as_str()
            .context("Selected workflow path missing")?
            .into(),
        run,
        attempt,
        event: "workflow_dispatch".into(),
    })
}

pub(crate) fn authenticate_preparation(
    app: &App,
    directory: &Path,
    identity: &crate::provenance::Identity,
    executor: &mut impl Executor,
) -> Result<(tempfile::TempDir, crate::store::Release)> {
    identity.validate()?;
    ensure!(
        identity.repository == repository(app)? && identity.event == "workflow_dispatch",
        "Native preparation must be dispatched from this repository"
    );
    let package = directory.join("native-release.zip");
    ensure!(
        package.is_file() && !package.is_symlink(),
        "Preparation package missing or unsafe"
    );
    verify_attestation(
        app,
        &package,
        Some(&directory.join(crate::provenance::BUNDLE)),
        &identity.source_sha,
        identity.run,
        identity.attempt,
        executor,
    )?;
    let statement = crate::provenance::verify(
        &package,
        &directory.join(crate::provenance::PROOF),
        identity,
        &app.root,
        executor,
    )?;
    let build = app.root.join("build");
    fsutil::confined(&app.root, &build)?;
    fs::create_dir_all(&build)?;
    let stage = tempfile::tempdir_in(build)?;
    let extracted = stage.path().join("release");
    crate::content::unpack(app, &package, &extracted)?;
    let release = crate::store::Release::load(app, &extracted)?;
    crate::provenance::validate_manifest(&release.manifest, identity)?;
    crate::provenance::validate_subjects(
        &statement,
        &crate::provenance::hashes(&extracted, &package)?,
    )?;
    Ok((stage, release))
}

pub fn verify_preparation(
    app: &App,
    directory: &Path,
    output: &Path,
    identity: &crate::provenance::Identity,
    executor: &mut impl Executor,
) -> Result<Value> {
    fsutil::confined(&app.root, directory)?;
    fsutil::confined(&app.root, output)?;
    ensure!(!output.exists(), "Verified release output already exists");
    let (_stage, release) = authenticate_preparation(app, directory, identity, executor)?;
    fs::create_dir_all(output.parent().context("Output needs a parent")?)?;
    fs::rename(&release.directory, output)?;
    Ok(release.manifest)
}
pub fn selection_output(manifest: &Value, path: &Path) -> Result<()> {
    let attempt = manifest["run_attempt"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .context("Verified preparation attempt missing")?;
    let mut output = fs::OpenOptions::new().append(true).open(path)?;
    writeln!(output, "preparation_attempt={attempt}")?;
    Ok(())
}
fn restore_receipt(
    app: &App,
    prepared_run: u64,
    prepared_attempt: u64,
    release: &mut crate::store::Release,
    executor: &mut impl Executor,
) -> Result<()> {
    let repo = repository(app)?;
    let prefix = format!("native-receipt-{prepared_run}-{prepared_attempt}-");
    let durable = crate::records::latest_receipt(app, prepared_run, prepared_attempt, executor)?;
    let mut found = vec![];
    for page in 1..=100 {
        let response = request(
            executor,
            &app.root,
            "GET",
            &format!("repos/{repo}/actions/artifacts?per_page=100&page={page}"),
            None,
        )?;
        let artifacts = response["artifacts"]
            .as_array()
            .context("GitHub artifacts response missing")?;
        found.extend(
            artifacts
                .iter()
                .filter(|a| {
                    a["expired"] == false
                        && a["name"].as_str().is_some_and(|n| n.starts_with(&prefix))
                })
                .cloned(),
        );
        if artifacts.len() < 100 {
            break;
        }
        ensure!(page < 100, "Receipt inventory exceeds its bound");
    }
    // Use the same operation order for durable records and live artifacts. A late
    // artifact upload from an older operation must not displace a newer receipt.
    let mut found = found
        .into_iter()
        .map(|artifact| {
            let name = artifact["name"].as_str().context("Receipt name missing")?;
            crate::config::filename(name)?;
            let (run, attempt) = name
                .strip_prefix(&prefix)
                .context("Receipt prefix differs")?
                .split_once('-')
                .context("Receipt attempt missing")?;
            let key = (run.parse::<u64>()?, attempt.parse::<u64>()?);
            ensure!(
                key.0 > 0 && key.1 > 0 && artifact["workflow_run"]["id"].as_u64() == Some(key.0),
                "Receipt artifact belongs to another operation run"
            );
            Ok((key, artifact))
        })
        .collect::<Result<Vec<_>>>()?;
    found.sort_by_key(|(key, _)| *key);
    let Some(((run, attempt), artifact)) = found.last() else {
        if let Some(selected) = &durable {
            crate::records::restore_receipt(
                app,
                release,
                prepared_run,
                prepared_attempt,
                selected,
                executor,
            )?;
        }
        return Ok(());
    };
    let (run, attempt) = (*run, *attempt);
    if let Some(selected) = &durable
        && (selected.0, selected.1) >= (run, attempt)
    {
        return crate::records::restore_receipt(
            app,
            release,
            prepared_run,
            prepared_attempt,
            selected,
            executor,
        );
    }
    let selected = request(
        executor,
        &app.root,
        "GET",
        &format!("repos/{repo}/actions/runs/{run}"),
        None,
    )?;
    ensure!(
        selected["repository"]["full_name"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(repo))
            && selected["event"] == "workflow_dispatch"
            && selected["head_branch"] == "main",
        "Receipt came from another repository or branch"
    );
    let source = selected["head_sha"]
        .as_str()
        .context("Receipt source missing")?;
    let name = artifact["name"].as_str().unwrap();
    let temp = download(app, run, name, executor)?;
    let file = temp.path().join("operation.json");
    verify_attestation(app, &file, None, source, run, attempt, executor)?;
    let value: Value = serde_json::from_slice(&fs::read(&file)?)?;
    ensure!(
        value["identity"] == release.receipt["identity"],
        "Saved operation receipt belongs to another app/IPA"
    );
    release.receipt = value;
    release.checkpoint(json!({"restored_from_run":run}))
}
