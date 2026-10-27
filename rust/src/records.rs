//! Immutable recovery records. Release metadata routes verification; it is never proof.
use crate::{
    config::App,
    fsutil, github,
    process::{Executor, Step, checked},
    provenance::{self, Identity},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

const RECORD: &str = "native-record.json";
pub const RECEIPT_BUNDLE: &str = "operation-attestation.jsonl";
const PREPARATION_FILES: [&str; 3] = ["native-release.zip", provenance::BUNDLE, provenance::PROOF];
const RECEIPT_FILES: [&str; 2] = ["operation.json", RECEIPT_BUNDLE];

pub fn preparation_tag(run: u64, attempt: u64) -> Result<String> {
    ensure!(
        run > 0 && attempt > 0,
        "Preparation run and attempt are required"
    );
    Ok(format!("native-preparation-{run}-{attempt}"))
}
fn receipt_tag(run: u64, attempt: u64, operation: u64, retry: u64) -> Result<String> {
    ensure!(
        operation > 0 && retry > 0,
        "Operation run and attempt are required"
    );
    preparation_tag(run, attempt)?;
    Ok(format!(
        "native-receipt-{run}-{attempt}-{operation}-{retry}"
    ))
}
fn api(
    app: &App,
    executor: &mut impl Executor,
    method: &str,
    suffix: &str,
    body: Option<&Value>,
) -> Result<Value> {
    github::request(
        executor,
        &app.root,
        method,
        &format!("repos/{}{suffix}", github::repository(app)?),
        body,
    )
}
pub fn releases(app: &App, executor: &mut impl Executor) -> Result<Vec<Value>> {
    let mut releases = Vec::new();
    for page in 1..=100 {
        let value = api(
            app,
            executor,
            "GET",
            &format!("/releases?per_page=100&page={page}"),
            None,
        )?;
        let entries = value.as_array().context("Release inventory missing")?;
        releases.extend(entries.iter().cloned());
        if entries.len() < 100 {
            return Ok(releases);
        }
    }
    anyhow::bail!("Release inventory exceeds its bound")
}
fn find(app: &App, tag: &str, executor: &mut impl Executor) -> Result<Option<Value>> {
    let matches = releases(app, executor)?
        .into_iter()
        .filter(|r| r["tag_name"] == tag)
        .collect::<Vec<_>>();
    ensure!(
        matches.len() <= 1,
        "Multiple releases claim the recovery tag"
    );
    Ok(matches.into_iter().next())
}
fn tag_commit(app: &App, tag: &str, executor: &mut impl Executor) -> Result<Option<String>> {
    let values = api(
        app,
        executor,
        "GET",
        &format!("/git/matching-refs/tags/{tag}"),
        None,
    )?;
    let refs = values
        .as_array()
        .context("Tag inventory missing")?
        .iter()
        .filter(|r| r["ref"] == format!("refs/tags/{tag}"))
        .collect::<Vec<_>>();
    ensure!(refs.len() <= 1, "Duplicate recovery tag");
    let Some(reference) = refs.first() else {
        return Ok(None);
    };
    let mut object = reference["object"].clone();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..16 {
        let sha = object["sha"].as_str().context("Tag object SHA missing")?;
        ensure!(
            sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid tag object SHA"
        );
        if object["type"] == "commit" {
            return Ok(Some(sha.into()));
        }
        ensure!(
            object["type"] == "tag" && seen.insert(sha.to_owned()),
            "Invalid or cyclic tag target"
        );
        object = api(app, executor, "GET", &format!("/git/tags/{sha}"), None)?["object"].clone();
    }
    anyhow::bail!("Annotated tag chain exceeds its bound")
}
fn assets(app: &App, id: u64, executor: &mut impl Executor) -> Result<BTreeMap<String, String>> {
    ensure!(id > 0, "Release operation ID missing");
    let mut files = BTreeMap::new();
    for page in 1..=2 {
        let value = api(
            app,
            executor,
            "GET",
            &format!("/releases/{id}/assets?per_page=100&page={page}"),
            None,
        )?;
        let entries = value.as_array().context("Release assets missing")?;
        for entry in entries {
            let name = entry["name"].as_str().context("Asset name missing")?;
            let digest = entry["digest"].as_str().context("Asset checksum missing")?;
            ensure!(
                entry["state"] == "uploaded" && files.insert(name.into(), digest.into()).is_none(),
                "Incomplete or duplicate release asset"
            );
        }
        if entries.len() < 100 {
            return Ok(files);
        }
    }
    anyhow::bail!("Recovery record has too many assets")
}
fn local_hashes(directory: &Path, names: &[&str]) -> Result<BTreeMap<String, String>> {
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            ensure!(
                path.is_file() && !path.is_symlink(),
                "Recovery asset missing or unsafe: {name}"
            );
            let bound = if *name == "native-release.zip" {
                8 * 1024 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            };
            ensure!(
                fs::metadata(&path)?.len() <= bound,
                "Recovery asset too large: {name}"
            );
            Ok(((*name).to_owned(), fsutil::sha256(&path)?))
        })
        .collect()
}
fn expected_assets(directory: &Path, names: &[&str]) -> Result<BTreeMap<String, String>> {
    Ok(local_hashes(directory, names)?
        .into_iter()
        .map(|(n, h)| (n, format!("sha256:{h}")))
        .collect())
}
fn validate_release(value: &Value, id: u64, tag: &str, published: bool) -> Result<()> {
    ensure!(
        value["id"].as_u64() == Some(id) && value["tag_name"] == tag && value["prerelease"] == true,
        "Recovery release identity or prerelease status changed"
    );
    if published {
        ensure!(
            value["draft"] == false && value["immutable"] == true,
            "Recovery requires a published immutable record"
        );
    } else {
        ensure!(
            value["draft"].as_bool().is_some(),
            "Release draft state missing"
        );
    }
    Ok(())
}

/// Called only after authenticating every consumed local byte. Never overwrites an asset or tag.
fn publish(
    app: &App,
    directory: &Path,
    record: &Value,
    executor: &mut impl Executor,
) -> Result<Value> {
    let tag = record["tag"].as_str().context("Record tag missing")?;
    let source = record["producer"]["source_sha"]
        .as_str()
        .context("Record source missing")?;
    let names = record["files"]
        .as_object()
        .context("Record assets missing")?
        .keys()
        .map(String::as_str)
        .chain([RECORD])
        .collect::<Vec<_>>();
    fsutil::json(&directory.join(RECORD), record)?;
    let expected = expected_assets(directory, &names)?;
    let marker = format!(
        "<!-- native-record-sha256:{} -->",
        fsutil::sha256(&directory.join(RECORD))?
    );
    ensure!(
        api(app, executor, "GET", "/immutable-releases", None)?["enabled"] == true,
        "Enable immutable releases before recording native recovery evidence"
    );
    let environment = api(
        app,
        executor,
        "GET",
        "/environments/release-publishing",
        None,
    )?;
    let branches = api(
        app,
        executor,
        "GET",
        "/environments/release-publishing/deployment-branch-policies",
        None,
    )?;
    ensure!(
        environment["deployment_branch_policy"]["custom_branch_policies"] == true
            && environment["deployment_branch_policy"]["protected_branches"] == false
            && branches["branch_policies"]
                .as_array()
                .is_some_and(|v| v.len() == 1
                    && v[0]["name"] == "main"
                    && v[0]["type"] == "branch"),
        "Release publishing must be restricted to main"
    );
    let existing = find(app, tag, executor)?;
    if let Some(value) = &existing {
        ensure!(
            value["body"]
                .as_str()
                .is_some_and(|body| body.ends_with(&marker)),
            "Existing record belongs to another handoff"
        );
    }
    match tag_commit(app, tag, executor)? {
        Some(sha) => ensure!(
            sha == source,
            "Recovery tag conflicts with the signed source"
        ),
        None => {
            ensure!(
                existing.as_ref().is_none_or(|r| r["draft"] == true),
                "Published record lost its tag"
            );
            api(
                app,
                executor,
                "POST",
                "/git/refs",
                Some(&json!({"ref":format!("refs/tags/{tag}"),"sha":source})),
            )?;
        }
    }
    ensure!(
        tag_commit(app, tag, executor)?.as_deref() == Some(source),
        "Recovery tag readback differs"
    );
    let value = match existing {
        Some(value) => value,
        None => api(
            app,
            executor,
            "POST",
            "/releases",
            Some(
                &json!({"tag_name":tag,"target_commitish":source,"draft":true,
            "prerelease":true,"make_latest":"false","name":tag,
            "body":format!("Authenticated native recovery evidence. This record does not submit or publish an app to Apple.\n\n{marker}")}),
            ),
        )?,
    };
    let id = value["id"]
        .as_u64()
        .context("Release operation ID missing")?;
    validate_release(&value, id, tag, false)?;
    let remote = assets(app, id, executor)?;
    ensure!(
        remote.iter().all(|(n, h)| expected.get(n) == Some(h)),
        "Conflicting or unexpected recovery assets; nothing was replaced"
    );
    for name in expected.keys().filter(|n| !remote.contains_key(*n)) {
        ensure!(
            value["draft"] == true,
            "Published recovery record is incomplete"
        );
        // Address the recorded operation ID, never resolve a possibly competing draft by tag.
        let url = format!(
            "https://uploads.github.com/repos/{}/releases/{id}/assets?name={name}",
            github::repository(app)?
        );
        checked(
            executor,
            &Step::new(
                "gh",
                [
                    "api",
                    "--hostname",
                    "github.com",
                    "--method",
                    "POST",
                    &url,
                    "--header",
                    "Content-Type: application/octet-stream",
                    "--input",
                    directory
                        .join(name)
                        .to_str()
                        .context("Invalid asset path")?,
                ],
                600,
            )
            .github(),
            &app.root,
        )?;
    }
    ensure!(
        assets(app, id, executor)? == expected,
        "Recovery asset readback differs"
    );
    ensure!(
        tag_commit(app, tag, executor)?.as_deref() == Some(source),
        "Recovery tag changed before publication"
    );
    let endpoint = format!("/releases/{id}");
    let readback = api(app, executor, "GET", &endpoint, None)?;
    validate_release(&readback, id, tag, false)?;
    ensure!(
        readback["body"]
            .as_str()
            .is_some_and(|s| s.ends_with(&marker)),
        "Recovery draft changed before publication"
    );
    if readback["draft"] == true {
        api(
            app,
            executor,
            "PATCH",
            &endpoint,
            Some(&json!({"draft":false,"make_latest":"false"})),
        )?;
    }
    let published = api(app, executor, "GET", &endpoint, None)?;
    validate_release(&published, id, tag, true)?;
    ensure!(
        assets(app, id, executor)? == expected
            && tag_commit(app, tag, executor)?.as_deref() == Some(source),
        "Published recovery record differs from authenticated evidence"
    );
    Ok(published)
}

pub fn record_preparation(
    app: &App,
    directory: &Path,
    identity: &Identity,
    executor: &mut impl Executor,
) -> Result<Value> {
    fsutil::confined(&app.root, directory)?;
    let (_stage, _release) = github::authenticate_preparation(app, directory, identity, executor)?;
    let tag = preparation_tag(identity.run, identity.attempt)?;
    let record = json!({"schema_version":1,"kind":"preparation","tag":tag,"producer":identity,
        "preparation_run":identity.run,"preparation_attempt":identity.attempt,"files":local_hashes(directory, &PREPARATION_FILES)?});
    publish(app, directory, &record, executor)
}

/// Optional lookup: only absence falls back to Actions; an invalid record is always fatal.
pub fn download_preparation(
    app: &App,
    identity: &Identity,
    executor: &mut impl Executor,
) -> Result<Option<tempfile::TempDir>> {
    let tag = preparation_tag(identity.run, identity.attempt)?;
    let Some(release) = find(app, &tag, executor)? else {
        return Ok(None);
    };
    let (directory, record) = download(app, &release, &tag, &PREPARATION_FILES, executor)?;
    ensure!(
        record["kind"] == "preparation"
            && record["producer"] == serde_json::to_value(identity)?
            && record["preparation_run"] == identity.run
            && record["preparation_attempt"] == identity.attempt,
        "Recovery record does not match the selected preparation"
    );
    Ok(Some(directory))
}
fn download(
    app: &App,
    release: &Value,
    tag: &str,
    files: &[&str],
    executor: &mut impl Executor,
) -> Result<(tempfile::TempDir, Value)> {
    let id = release["id"]
        .as_u64()
        .context("Recovery release ID missing")?;
    validate_release(release, id, tag, true)?;
    let temp = tempfile::tempdir()?;
    let names = files.iter().copied().chain([RECORD]).collect::<Vec<_>>();
    let remote = assets(app, id, executor)?;
    ensure!(
        remote
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>()
            == names.iter().copied().collect(),
        "Recovery asset set differs"
    );
    let mut args = vec![
        "release".to_owned(),
        "download".into(),
        tag.into(),
        "--repo".into(),
        github::repository(app)?.into(),
        "--dir".into(),
        temp.path().to_str().unwrap().into(),
    ];
    for name in &names {
        args.extend(["--pattern".into(), (*name).into()]);
    }
    checked(executor, &Step::new("gh", args, 600).github(), &app.root)?;
    ensure!(
        expected_assets(temp.path(), &names)? == remote,
        "Downloaded recovery asset checksum differs"
    );
    let record: Value = serde_json::from_slice(&fs::read(temp.path().join(RECORD))?)?;
    ensure!(
        record["schema_version"] == 1
            && record["tag"] == tag
            && record["files"] == serde_json::to_value(local_hashes(temp.path(), files)?)?,
        "Recovery record inventory differs"
    );
    let producer: Identity = serde_json::from_value(record["producer"].clone())?;
    producer.validate()?;
    ensure!(
        producer.repository == github::repository(app)?
            && producer.event == "workflow_dispatch"
            && tag_commit(app, tag, executor)?.as_deref() == Some(&producer.source_sha),
        "Recovery tag or repository differs"
    );
    let comparison = api(
        app,
        executor,
        "GET",
        &format!("/compare/{}...main", producer.source_sha),
        None,
    )?;
    ensure!(
        ["ahead", "identical"].contains(&comparison["status"].as_str().unwrap_or("")),
        "Recovery source is not an ancestor of main"
    );
    Ok((temp, record))
}

pub fn record_receipt(
    app: &App,
    directory: &Path,
    prepared_run: u64,
    prepared_attempt: u64,
    producer: &Identity,
    executor: &mut impl Executor,
) -> Result<Value> {
    producer.validate()?;
    ensure!(
        producer.repository == github::repository(app)? && producer.event == "workflow_dispatch",
        "Receipt producer differs"
    );
    fsutil::confined(&app.root, directory)?;
    // Reauthenticate the preparation before accepting any receipt identity.
    let selected = github::preparation_identity(app, prepared_run, executor)?;
    ensure!(
        selected.attempt == prepared_attempt,
        "Receipt refers to another preparation attempt"
    );
    let preparation = download_preparation(app, &selected, executor)?
        .context("Durable preparation missing; record it before its receipt")?;
    let (_stage, release) =
        github::authenticate_preparation(app, preparation.path(), &selected, executor)?;
    verify_receipt(app, directory, producer, &release, executor)?;
    let tag = receipt_tag(
        prepared_run,
        prepared_attempt,
        producer.run,
        producer.attempt,
    )?;
    let record = json!({"schema_version":1,"kind":"receipt","tag":tag,"producer":producer,
        "preparation_run":prepared_run,"preparation_attempt":prepared_attempt,"files":local_hashes(directory, &RECEIPT_FILES)?});
    publish(app, directory, &record, executor)
}
fn verify_receipt(
    app: &App,
    directory: &Path,
    producer: &Identity,
    release: &crate::store::Release,
    executor: &mut impl Executor,
) -> Result<Value> {
    local_hashes(directory, &RECEIPT_FILES)?;
    github::verify_attestation(
        app,
        &directory.join("operation.json"),
        Some(&directory.join(RECEIPT_BUNDLE)),
        &producer.source_sha,
        producer.run,
        producer.attempt,
        executor,
    )?;
    let receipt: Value = serde_json::from_slice(&fs::read(directory.join("operation.json"))?)?;
    ensure!(
        receipt["identity"] == release.receipt["identity"],
        "Saved operation belongs to another preparation or IPA"
    );
    Ok(receipt)
}
pub fn latest_receipt(
    app: &App,
    run: u64,
    attempt: u64,
    executor: &mut impl Executor,
) -> Result<Option<(u64, u64, Value)>> {
    let prefix = format!("native-receipt-{run}-{attempt}-");
    let mut found = BTreeMap::new();
    for release in releases(app, executor)? {
        let Some(suffix) = release["tag_name"]
            .as_str()
            .and_then(|s| s.strip_prefix(&prefix))
        else {
            continue;
        };
        // A partially uploaded draft cannot erase a previously published checkpoint.
        if release["draft"] == true {
            continue;
        }
        let (operation, retry) = suffix
            .split_once('-')
            .context("Invalid receipt record tag")?;
        let key = (operation.parse::<u64>()?, retry.parse::<u64>()?);
        ensure!(
            key.0 > 0 && key.1 > 0 && found.insert(key, release).is_none(),
            "Ambiguous receipt record"
        );
    }
    Ok(found
        .pop_last()
        .map(|((run, attempt), value)| (run, attempt, value)))
}
pub fn restore_receipt(
    app: &App,
    release: &mut crate::store::Release,
    prepared_run: u64,
    prepared_attempt: u64,
    selected: &(u64, u64, Value),
    executor: &mut impl Executor,
) -> Result<()> {
    let tag = receipt_tag(prepared_run, prepared_attempt, selected.0, selected.1)?;
    let (directory, record) = download(app, &selected.2, &tag, &RECEIPT_FILES, executor)?;
    let producer: Identity = serde_json::from_value(record["producer"].clone())?;
    ensure!(
        record["kind"] == "receipt"
            && record["preparation_run"] == prepared_run
            && record["preparation_attempt"] == prepared_attempt
            && producer.run == selected.0
            && producer.attempt == selected.1,
        "Recovery receipt selection differs"
    );
    release.receipt = verify_receipt(app, directory.path(), &producer, release, executor)?;
    release.checkpoint(json!({"restored_from_record":tag}))
}
