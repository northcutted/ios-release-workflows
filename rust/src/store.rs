use crate::{
    api::{self, Api},
    config::App,
    fsutil,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn app_id(app: &App, api: &mut impl Api) -> Result<String> {
    let bundle = app.config["app_store"]["bundle_id"]
        .as_str()
        .or_else(|| app.config["targets"][0]["bundle_id"].as_str())
        .context("App bundle ID is missing")?;
    let values = api::list(
        api,
        "/v1/apps",
        &api::query(&[("filter[bundleId]", bundle)]),
    )?;
    ensure!(
        values.len() == 1,
        "Create this app's record once at https://appstoreconnect.apple.com/apps using bundle ID {bundle}, then rerun this command"
    );
    ensure!(
        values[0]["attributes"]["bundleId"] == bundle,
        "Apple app identity differs"
    );
    Ok(api::id(&values[0])?.to_owned())
}
pub fn status(app: &App, api: &mut impl Api) -> Result<Value> {
    let id = app_id(app, api)?;
    let versions = api::list(
        api,
        &format!("/v1/apps/{id}/appStoreVersions"),
        &api::query(&[("filter[platform]", "IOS"), ("limit", "200")]),
    )?;
    let builds = api::list(
        api,
        "/v1/builds",
        &api::query(&[
            ("filter[app]", &id),
            ("sort", "-uploadedDate"),
            ("limit", "10"),
        ]),
    )?;
    Ok(
        json!({"app_id":id,"bundle_id":app.config["app_store"]["bundle_id"],"versions":versions.iter().map(|v|json!({"id":v["id"],"version":v["attributes"]["versionString"],"state":v["attributes"]["appVersionState"].as_str().or(v["attributes"]["appStoreState"].as_str()),"release_type":v["attributes"]["releaseType"]})).collect::<Vec<_>>(),"builds":builds.iter().take(10).map(|v|json!({"id":v["id"],"build_number":v["attributes"]["version"],"processing":v["attributes"]["processingState"],"expires":v["attributes"]["expirationDate"]})).collect::<Vec<_>>()}),
    )
}
pub fn next_number(app: &App, api: &mut impl Api) -> Result<String> {
    let id = app_id(app, api)?;
    let builds = api::list(
        api,
        "/v1/builds",
        &api::query(&[("filter[app]", &id), ("limit", "200")]),
    )?;
    let highest =
        builds
            .iter()
            .filter_map(|b| b["attributes"]["version"].as_str())
            .map(|v| {
                v.split('.').next().unwrap_or("").parse::<u64>().context(
                    "Apple has an unsupported build number; pass --build-number explicitly",
                )
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
    Ok(highest
        .checked_add(1)
        .context("Build number overflow")?
        .to_string())
}
pub fn beta_groups(
    app: &mut App,
    api: &mut impl Api,
    create: Option<&str>,
    external: bool,
) -> Result<Value> {
    let app_id = app_id(app, api)?;
    let groups = api::list(
        api,
        &format!("/v1/apps/{app_id}/betaGroups"),
        &api::query(&[("limit", "200")]),
    )?;
    if let Some(name) = create {
        ensure!(
            !name.trim().is_empty() && name.len() <= 100 && !name.contains(['\n', '\r', '\0']),
            "Choose a nonempty beta group name"
        );
        let matching = groups
            .iter()
            .filter(|g| g["attributes"]["name"] == name)
            .collect::<Vec<_>>();
        ensure!(
            matching.len() <= 1,
            "More than one beta group has this name; select it in App Store Connect"
        );
        let group = if let Some(existing) = matching.first() {
            (*existing).clone()
        } else {
            api.request("POST", "/v1/betaGroups", &[], Some(&json!({"data":{"type":"betaGroups","attributes":{"name":name,"isInternalGroup":!external,"hasAccessToAllBuilds":false,"publicLinkEnabled":false},"relationships":{"app":{"data":{"type":"apps","id":app_id}}}}})))?["data"].clone()
        };
        let id = api::id(&group)?;
        let actual = api.request(
            "GET",
            &format!("/v1/betaGroups/{id}"),
            &api::query(&[("include", "app")]),
            None,
        )?;
        ensure!(
            actual["data"]["relationships"]["app"]["data"]["id"] == app_id
                && actual["data"]["attributes"]["name"] == name
                && actual["data"]["attributes"]["isInternalGroup"] == !external,
            "Beta group readback differs from the requested app/name/type"
        );
        if app.config["app_store"]["testflight_groups"].is_null() {
            app.config["app_store"]["testflight_groups"] = json!([]);
        }
        let configured = app.config["app_store"]["testflight_groups"]
            .as_array_mut()
            .context("Configured TestFlight groups must be a list")?;
        if !configured.iter().any(|v| v == id) {
            configured.push(json!(id));
        }
        app.save()?;
        println!(
            "Configured beta group {name}. Commit the app configuration before preparing a release. Manage testers in https://appstoreconnect.apple.com/apps/{app_id}/testflight."
        );
        Ok(json!({"id":id,"name":name,"external":external}))
    } else {
        Ok(
            json!({"groups":groups.iter().map(|g| json!({"id":g["id"],"name":g["attributes"]["name"],"internal":g["attributes"]["isInternalGroup"],"public_link":g["attributes"]["publicLink"]})).collect::<Vec<_>>() }),
        )
    }
}

pub struct Release {
    pub directory: PathBuf,
    pub manifest: Value,
    pub receipt: Value,
    pub app_config: Value,
}
impl Release {
    pub fn load(app: &App, directory: &Path) -> Result<Self> {
        let directory = directory
            .canonicalize()
            .context("Release directory does not exist")?;
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(directory.join("release.json"))?)?;
        ensure!(
            manifest["schema_version"] == 2 && manifest["verified"] == true,
            "Release has not passed native preparation"
        );
        ensure!(
            manifest["bundle_id"] == app.config["app_store"]["bundle_id"]
                && manifest["team_id"] == app.config["team_id"],
            "Release belongs to another app or Apple team"
        );
        ensure!(
            manifest["version"].as_str().is_some() && manifest["build_number"].as_str().is_some(),
            "Release identity missing"
        );
        ensure!(
            manifest["ipa_sha256"].as_str()
                == Some(&fsutil::sha256(&directory.join("application.ipa"))?),
            "Release IPA checksum changed"
        );
        ensure!(
            manifest["config_sha256"].as_str()
                == Some(&fsutil::sha256(&directory.join("app.json"))?),
            "Release configuration changed"
        );
        for (file, key) in [("qa.json", "qa_sha256"), ("archive.json", "archive_sha256")] {
            ensure!(
                manifest[key].as_str() == Some(&fsutil::sha256(&directory.join(file))?),
                "Release {file} changed"
            );
        }
        let archived: Value =
            serde_json::from_slice(&std::fs::read(directory.join("archive.json"))?)?;
        let qa: Value = serde_json::from_slice(&std::fs::read(directory.join("qa.json"))?)?;
        let sealed_app = App::load(&directory, Path::new("app.json"))?;
        crate::release::validate_inventory(&sealed_app, &archived)?;
        ensure!(
            sealed_app.config["team_id"] == manifest["team_id"]
                && sealed_app.config["app_store"]["bundle_id"] == manifest["bundle_id"],
            "Sealed configuration belongs to another app/team"
        );
        ensure!(
            archived["source_sha"].as_str().unwrap_or("local") == manifest["source_sha"]
                && archived["run_id"] == manifest["run_id"],
            "Manifest and archive source/run identities differ"
        );
        ensure!(
            archived["version"] == manifest["version"]
                && archived["build_number"] == manifest["build_number"]
                && archived["ipa_sha256"] == manifest["ipa_sha256"],
            "Archive evidence belongs to another binary"
        );
        ensure!(
            archived["inputs_sha256"]
                .as_str()
                .is_some_and(|s| s.len() == 64),
            "Archive input identity missing"
        );
        let checks = qa.as_object().context("Release QA evidence missing")?;
        ensure!(
            sealed_app
                .qa_checks()?
                .iter()
                .all(|c| checks.contains_key(c)),
            "Release has missing configured QA evidence"
        );
        for (name, value) in checks {
            ensure!(
                value["status"] == 0
                    && value["inputs_sha256"] == archived["inputs_sha256"]
                    && value["source_sha"] == archived["source_sha"]
                    && value["run_id"] == archived["run_id"],
                "Release QA {name} failed or has a different identity"
            );
            if name.starts_with("test") {
                let report = crate::results::junit(&value["summary"], &value["tests"])?;
                ensure!(
                    report.passed && report.executed > 0,
                    "Release app tests did not execute successfully"
                );
            }
        }
        let receipt = if directory.join("operation.json").exists() {
            serde_json::from_slice(&std::fs::read(directory.join("operation.json"))?)?
        } else {
            json!({"identity":{"bundle_id":manifest["bundle_id"],"version":manifest["version"],"build_number":manifest["build_number"],"ipa_sha256":manifest["ipa_sha256"]}})
        };
        ensure!(
            receipt["identity"]
                == json!({"bundle_id":manifest["bundle_id"],"version":manifest["version"],"build_number":manifest["build_number"],"ipa_sha256":manifest["ipa_sha256"]}),
            "Operation receipt belongs to another release"
        );
        Ok(Self {
            directory,
            manifest,
            receipt,
            app_config: sealed_app.config,
        })
    }
    pub fn checkpoint(&mut self, values: Value) -> Result<()> {
        for (key, value) in values.as_object().context("Invalid operation checkpoint")? {
            self.receipt[key] = value.clone();
        }
        fsutil::json(&self.directory.join("operation.json"), &self.receipt)
    }
    fn version(&self) -> &str {
        self.manifest["version"].as_str().unwrap()
    }
    fn number(&self) -> &str {
        self.manifest["build_number"].as_str().unwrap()
    }
    fn checksum(&self) -> &str {
        self.manifest["ipa_sha256"].as_str().unwrap()
    }
}
fn exact_upload(api: &mut impl Api, app_id: &str, release: &Release) -> Result<Option<Value>> {
    let uploads = api::list(
        api,
        &format!("/v1/apps/{app_id}/buildUploads"),
        &api::query(&[
            ("filter[cfBundleShortVersionString]", release.version()),
            ("filter[cfBundleVersion]", release.number()),
            ("filter[platform]", "IOS"),
        ]),
    )?;
    ensure!(
        uploads.len() <= 1,
        "Ambiguous Apple upload for this version/build number"
    );
    let upload = uploads.into_iter().next();
    if let Some(upload) = &upload {
        ensure!(
            upload["attributes"]["cfBundleShortVersionString"] == release.manifest["version"]
                && upload["attributes"]["cfBundleVersion"] == release.manifest["build_number"]
                && upload["attributes"]["platform"] == "IOS",
            "Apple upload identity changed"
        );
        ensure!(
            upload["attributes"]["state"]["state"] != "FAILED",
            "Apple rejected this upload; inspect the upload in App Store Connect"
        );
        if let Some(recorded) = release.receipt["upload_id"].as_str() {
            ensure!(api::id(upload)? == recorded, "Recorded upload ID changed");
        }
    }
    Ok(upload)
}
fn upload_file(api: &mut impl Api, upload: &Value) -> Result<Option<Value>> {
    let value = api.request(
        "GET",
        &format!("/v1/buildUploads/{}", api::id(upload)?),
        &api::query(&[("include", "assetFile,build")]),
        None,
    )?;
    let id = value["data"]["relationships"]["assetFile"]["data"]["id"].as_str();
    Ok(value["included"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["type"] == "buildUploadFiles" && v["id"].as_str() == id)
        .cloned())
}
fn checksum_matches(file: &Value, release: &Release) -> bool {
    file["attributes"]["sourceFileChecksums"]["file"]
        == json!({"algorithm":"SHA_256","hash":release.checksum()})
}
pub fn upload(app: &App, release: &mut Release, api: &mut impl Api) -> Result<()> {
    let app_id = app_id(app, api)?;
    let existing = exact_upload(api, &app_id, release)?;
    let upload = if let Some(upload) = existing {
        // A reservation without a local intent may belong to another uploader.
        // Complete assets can be adopted only after comparing their exact digest.
        if release.receipt["upload_id"].is_null() && release.receipt["app_id"].is_null() {
            let file = upload_file(api, &upload)?
                .context("Unowned unfinished upload exists; use a new build number")?;
            ensure!(
                checksum_matches(&file, release),
                "Unowned upload reservation exists; use a new build number"
            );
        }
        upload
    } else {
        let builds = api::list(
            api,
            "/v1/builds",
            &api::query(&[
                ("filter[app]", &app_id),
                ("filter[version]", release.number()),
            ]),
        )?;
        ensure!(
            builds.is_empty(),
            "This build number already exists without a matching native upload receipt; select its preparation or use a new build number"
        );
        release.checkpoint(json!({"status":"upload-intent","app_id":app_id}))?;
        api.request("POST","/v1/buildUploads",&[],Some(&json!({"data":{"type":"buildUploads","attributes":{"cfBundleShortVersionString":release.version(),"cfBundleVersion":release.number(),"platform":"IOS"},"relationships":{"app":{"data":{"type":"apps","id":app_id}}}}})))?["data"].clone()
    };
    release.checkpoint(json!({"upload_id":api::id(&upload)?}))?;
    let ipa = release.directory.join("application.ipa");
    let file = if let Some(file) = upload_file(api, &upload)? {
        file
    } else {
        api.request("POST","/v1/buildUploadFiles",&[],Some(&json!({"data":{"type":"buildUploadFiles","attributes":{"assetType":"ASSET","fileName":"application.ipa","fileSize":ipa.metadata()?.len(),"uti":"com.apple.ipa"},"relationships":{"buildUpload":{"data":{"type":"buildUploads","id":api::id(&upload)?}}}}})))?["data"].clone()
    };
    let file_id = api::id(&file)?.to_owned();
    if let Some(recorded) = release.receipt["upload_file_id"].as_str() {
        ensure!(recorded == file_id, "Upload file identity changed");
    }
    ensure!(
        file["attributes"]["fileSize"].as_u64() == Some(ipa.metadata()?.len()),
        "Upload file size changed"
    );
    release.checkpoint(json!({"upload_file_id":file_id}))?;
    let state = file["attributes"]["assetDeliveryState"]["state"]
        .as_str()
        .context("Missing asset delivery state")?;
    if ["UPLOAD_COMPLETE", "COMPLETE"].contains(&state) {
        ensure!(
            checksum_matches(&file, release),
            "Existing upload bytes differ"
        );
    } else {
        ensure!(
            state == "AWAITING_UPLOAD",
            "Apple upload file is not resumable in state {state}"
        );
        api.transfer(&ipa, &file["attributes"]["uploadOperations"])?;
        api.request("PATCH",&format!("/v1/buildUploadFiles/{file_id}"),&[],Some(&json!({"data":{"type":"buildUploadFiles","id":file_id,"attributes":{"uploaded":true,"sourceFileChecksums":{"file":{"algorithm":"SHA_256","hash":release.checksum()}}}}})))?;
    }
    release.checkpoint(json!({"status":"transferred"}))?;
    Ok(())
}
pub fn processed(app: &App, release: &mut Release, api: &mut impl Api) -> Result<Option<Value>> {
    let app_id = app_id(app, api)?;
    let upload = exact_upload(api, &app_id, release)?
        .context("No upload exists for this release; run store upload")?;
    let Some(file) = upload_file(api, &upload)? else {
        return Ok(None);
    };
    ensure!(
        checksum_matches(&file, release),
        "Apple upload checksum does not authenticate this IPA"
    );
    if let Some(recorded) = release.receipt["upload_file_id"].as_str() {
        ensure!(api::id(&file)? == recorded, "Upload file identity changed");
    }
    let expanded = api.request(
        "GET",
        &format!("/v1/buildUploads/{}", api::id(&upload)?),
        &api::query(&[("include", "build")]),
        None,
    )?;
    let Some(build_id) = expanded["data"]["relationships"]["build"]["data"]["id"].as_str() else {
        return Ok(None);
    };
    let expanded_build = api.request(
        "GET",
        &format!("/v1/builds/{build_id}"),
        &api::query(&[("include", "app,preReleaseVersion")]),
        None,
    )?;
    let build = expanded_build["data"].clone();
    let prerelease_id = build["relationships"]["preReleaseVersion"]["data"]["id"]
        .as_str()
        .context("Build has no marketing version relation")?;
    let prerelease = expanded_build["included"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["type"] == "preReleaseVersions" && v["id"] == prerelease_id)
        .context("Apple did not include the build marketing version")?;
    ensure!(
        prerelease["attributes"]["version"] == release.manifest["version"]
            && prerelease["attributes"]["platform"] == "IOS",
        "Processed build belongs to another marketing version/platform"
    );
    ensure!(
        build["attributes"]["version"] == release.manifest["build_number"]
            && build["relationships"]["app"]["data"]["id"] == app_id,
        "Processed build belongs to another app/build"
    );
    if let Some(recorded) = release.receipt["build_id"].as_str() {
        ensure!(recorded == build_id, "Processed build identity changed");
    }
    match build["attributes"]["processingState"].as_str() {
        Some("VALID") => {
            ensure!(
                upload["attributes"]["state"]["state"] == "COMPLETE",
                "Build processing completed before its upload became complete; resume store wait"
            );
            release.checkpoint(json!({"build_id":build_id,"status":"processed"}))?;
            Ok(Some(build))
        }
        Some("PROCESSING") => Ok(None),
        _ => bail!("Apple processing failed; inspect App Store Connect for this exact build"),
    }
}
pub fn wait(app: &App, release: &mut Release, api: &mut impl Api, timeout: u64) -> Result<Value> {
    ensure!(
        timeout > 0 && timeout <= 7200,
        "Processing wait must be between 1 and 7200 seconds"
    );
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        if let Some(build) = processed(app, release, api)? {
            return Ok(build);
        }
        ensure!(
            Instant::now() < deadline,
            "Apple is still processing. The transfer receipt is saved; resume with store wait"
        );
        std::thread::sleep(
            Duration::from_secs(15).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
fn version_record(app: &App, release: &Release, api: &mut impl Api) -> Result<Option<Value>> {
    let app_id = app_id(app, api)?;
    let versions = api::list(
        api,
        &format!("/v1/apps/{app_id}/appStoreVersions"),
        &api::query(&[
            ("filter[platform]", "IOS"),
            ("filter[versionString]", release.version()),
        ]),
    )?;
    ensure!(versions.len() <= 1, "Ambiguous App Store version");
    Ok(versions.into_iter().next())
}
fn state(version: &Value) -> Result<&str> {
    version["attributes"]["appVersionState"]
        .as_str()
        .or(version["attributes"]["appStoreState"].as_str())
        .context("Missing App Store version state")
}
pub fn stage(app: &App, release: &mut Release, api: &mut impl Api) -> Result<String> {
    let build =
        processed(app, release, api)?.context("Build processing is incomplete; run store wait")?;
    let app_id = app_id(app, api)?;
    let version = if let Some(v) = version_record(app, release, api)? {
        v
    } else {
        api.request("POST","/v1/appStoreVersions",&[],Some(&json!({"data":{"type":"appStoreVersions","attributes":{"platform":"IOS","versionString":release.version(),"releaseType":release.app_config["app_store"]["release_type"].as_str().unwrap_or("MANUAL")},"relationships":{"app":{"data":{"type":"apps","id":app_id}}}}})))?["data"].clone()
    };
    let version_id = api::id(&version)?.to_owned();
    ensure!(
        version["attributes"]["versionString"] == release.manifest["version"]
            && version["attributes"]["platform"] == "IOS",
        "App Store version identity differs"
    );
    let selected = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{version_id}/build"),
        &[],
        None,
    )?["data"]
        .clone();
    if !selected.is_null() {
        ensure!(
            selected["id"] == build["id"],
            "A different build is already selected; this CLI will not replace it implicitly"
        );
    } else {
        ensure!(
            state(&version)? == "PREPARE_FOR_SUBMISSION",
            "Version is not editable"
        );
        api.request(
            "PATCH",
            &format!("/v1/appStoreVersions/{version_id}/relationships/build"),
            &[],
            Some(&json!({"data":{"type":"builds","id":api::id(&build)?}})),
        )?;
    }
    let readback = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{version_id}/build"),
        &[],
        None,
    )?;
    ensure!(
        readback["data"]["id"] == build["id"],
        "Selected build readback differs"
    );
    release.checkpoint(json!({"version_id":version_id,"status":"staged"}))?;
    apply_release_policy(&release.app_config, &version, api)?;
    encryption(release, &build, api)?;
    Ok(version_id)
}
pub fn submit(app: &App, release: &mut Release, api: &mut impl Api, confirmed: bool) -> Result<()> {
    ensure!(
        confirmed,
        "Review the release with store status, then pass --confirm to request App Review"
    );
    let version_id = release.receipt["version_id"]
        .as_str()
        .context("Stage this exact release before submission")?
        .to_owned();
    let version = version_record(app, release, api)?.context("App Store version missing")?;
    ensure!(
        api::id(&version)? == version_id,
        "Staged version identity changed"
    );
    let build = processed(app, release, api)?.context("Build is not ready")?;
    let selected = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{version_id}/build"),
        &[],
        None,
    )?;
    ensure!(
        selected["data"]["id"] == build["id"],
        "Selected build differs from the release receipt"
    );
    if [
        "WAITING_FOR_REVIEW",
        "IN_REVIEW",
        "PENDING_DEVELOPER_RELEASE",
        "READY_FOR_SALE",
        "READY_FOR_DISTRIBUTION",
    ]
    .contains(&state(&version)?)
    {
        release.checkpoint(json!({"status":"submitted","readback_state":state(&version)?}))?;
        return Ok(());
    }
    let app_id = app_id(app, api)?;
    let submissions = api::list(
        api,
        &format!("/v1/apps/{app_id}/reviewSubmissions"),
        &api::query(&[("filter[platform]", "IOS")]),
    )?;
    let active = submissions
        .into_iter()
        .filter(|v| {
            !["COMPLETE", "CANCELED"].contains(&v["attributes"]["state"].as_str().unwrap_or(""))
        })
        .collect::<Vec<_>>();
    ensure!(
        active.len() <= 1,
        "Multiple active review submissions require an owner decision"
    );
    let submission = if let Some(value) = active.first() {
        if let Some(recorded) = release.receipt["review_submission_id"].as_str() {
            ensure!(
                api::id(value)? == recorded,
                "Active review submission changed"
            );
        } else {
            ensure!(
                release.receipt["review_intent"] == true,
                "An active review submission exists outside this release receipt; review it in App Store Connect before creating another release"
            );
        }
        value.clone()
    } else {
        release.checkpoint(json!({"status":"review-intent","review_intent":true}))?;
        api.request("POST","/v1/reviewSubmissions",&[],Some(&json!({"data":{"type":"reviewSubmissions","attributes":{"platform":"IOS"},"relationships":{"app":{"data":{"type":"apps","id":app_id}}}}})))?["data"].clone()
    };
    let submission_id = api::id(&submission)?.to_owned();
    release.checkpoint(json!({"review_submission_id":submission_id}))?;
    let items = api::list(
        api,
        &format!("/v1/reviewSubmissions/{submission_id}/items"),
        &api::query(&[("include", "appStoreVersion")]),
    )?;
    ensure!(
        items.len() <= 1
            && items
                .iter()
                .all(|i| i["relationships"]["appStoreVersion"]["data"]["id"] == version_id),
        "Review submission contains another release"
    );
    if items.is_empty() {
        api.request("POST","/v1/reviewSubmissionItems",&[],Some(&json!({"data":{"type":"reviewSubmissionItems","relationships":{"reviewSubmission":{"data":{"type":"reviewSubmissions","id":submission_id}},"appStoreVersion":{"data":{"type":"appStoreVersions","id":version_id}}}}})))?;
    }
    let ready = api.request(
        "GET",
        &format!("/v1/reviewSubmissions/{submission_id}"),
        &[],
        None,
    )?["data"]
        .clone();
    let review_state = ready["attributes"]["state"]
        .as_str()
        .context("Review state missing")?;
    ensure!(
        ["READY_FOR_REVIEW", "WAITING_FOR_REVIEW", "IN_REVIEW"].contains(&review_state),
        "Apple review checklist is incomplete. Open https://appstoreconnect.apple.com/apps/{app_id}/distribution and complete the listed account, privacy or store requirements; rerun store submit"
    );
    if review_state == "READY_FOR_REVIEW" {
        api.request("PATCH",&format!("/v1/reviewSubmissions/{submission_id}"),&[],Some(&json!({"data":{"type":"reviewSubmissions","id":submission_id,"attributes":{"submitted":true}}})))?;
    }
    let actual = api.request(
        "GET",
        &format!("/v1/reviewSubmissions/{submission_id}"),
        &[],
        None,
    )?;
    ensure!(
        ["WAITING_FOR_REVIEW", "IN_REVIEW", "COMPLETE"]
            .contains(&actual["data"]["attributes"]["state"].as_str().unwrap_or("")),
        "Apple has not confirmed review submission. Resume using the saved receipt"
    );
    release.checkpoint(
        json!({"status":"submitted","review_state":actual["data"]["attributes"]["state"]}),
    )
}
pub fn testflight(app: &App, release: &mut Release, api: &mut impl Api) -> Result<()> {
    let build = processed(app, release, api)?.context("Build is not ready for TestFlight")?;
    let app_id = app_id(app, api)?;
    encryption(release, &build, api)?;
    let mut external = false;
    for group in app.config["app_store"]["testflight_groups"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let group = group
            .as_str()
            .context("TestFlight groups must be Apple resource IDs")?;
        ensure!(
            group.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "Invalid TestFlight group ID"
        );
        let response = api.request(
            "GET",
            &format!("/v1/betaGroups/{group}"),
            &api::query(&[("include", "app")]),
            None,
        )?;
        ensure!(
            response["data"]["relationships"]["app"]["data"]["id"] == app_id,
            "TestFlight group belongs to another app"
        );
        external |= response["data"]["attributes"]["isInternalGroup"] == false;
        let existing = api::list(
            api,
            &format!("/v1/betaGroups/{group}/builds"),
            &api::query(&[("limit", "200")]),
        )?;
        if !existing.iter().any(|b| b["id"] == build["id"]) {
            api.request(
                "POST",
                &format!("/v1/betaGroups/{group}/relationships/builds"),
                &[],
                Some(&json!({"data":[{"type":"builds","id":api::id(&build)?}]})),
            )?;
        }
        ensure!(
            api::list(
                api,
                &format!("/v1/betaGroups/{group}/builds"),
                &api::query(&[("limit", "200")])
            )?
            .iter()
            .any(|b| b["id"] == build["id"]),
            "TestFlight assignment readback differs"
        );
    }
    if external {
        let build_id = api::id(&build)?;
        let review = api::optional(
            api,
            &format!("/v1/builds/{build_id}/betaAppReviewSubmission"),
            &[],
        )?;
        if review.as_ref().is_none_or(|v| v["data"].is_null()) {
            release.checkpoint(json!({"beta_review_intent":true}))?;
            let created=api.request("POST","/v1/betaAppReviewSubmissions",&[],Some(&json!({"data":{"type":"betaAppReviewSubmissions","relationships":{"build":{"data":{"type":"builds","id":build_id}}}}})))?;
            release.checkpoint(json!({"beta_review_id":api::id(&created["data"])?}))?;
        }
        let actual = api.request(
            "GET",
            &format!("/v1/builds/{build_id}/betaAppReviewSubmission"),
            &[],
            None,
        )?;
        ensure!(
            actual["data"]["id"].as_str().is_some(),
            "External beta review is not yet visible; resume store testflight using its saved receipt"
        );
        release.checkpoint(
            json!({"beta_review_state":actual["data"]["attributes"]["betaReviewState"]}),
        )?;
    }
    release
        .checkpoint(json!({"status":if external{"testflight-review-requested"}else{"testflight"}}))
}

fn encryption(release: &Release, build: &Value, api: &mut impl Api) -> Result<()> {
    let expected = release.app_config["targets"][0]["non_exempt_encryption"]
        .as_bool()
        .context("Declare the app encryption policy before delivery")?;
    if build["attributes"]["usesNonExemptEncryption"].as_bool() == Some(expected) {
        return Ok(());
    }
    ensure!(
        !expected,
        "Attach the required encryption declaration to this build in App Store Connect before delivery"
    );
    let id = api::id(build)?;
    api.request("PATCH",&format!("/v1/builds/{id}"),&[],Some(&json!({"data":{"type":"builds","id":id,"attributes":{"usesNonExemptEncryption":false}}})))?;
    let actual = api.request("GET", &format!("/v1/builds/{id}"), &[], None)?;
    ensure!(
        actual["data"]["attributes"]["usesNonExemptEncryption"] == false,
        "Encryption declaration readback differs"
    );
    Ok(())
}
fn apply_release_policy(config: &Value, version: &Value, api: &mut impl Api) -> Result<()> {
    let id = api::id(version)?;
    let kind = config["app_store"]["release_type"]
        .as_str()
        .unwrap_or("MANUAL");
    ensure!(
        ["MANUAL", "AFTER_APPROVAL", "SCHEDULED"].contains(&kind),
        "Invalid release_type; choose MANUAL, AFTER_APPROVAL or SCHEDULED"
    );
    let mut attributes = json!({"releaseType":kind});
    if let Some(value) = config["app_store"]["copyright"].as_str() {
        ensure!(
            !value.is_empty() && value.len() <= 1000,
            "Set a valid App Store copyright"
        );
        attributes["copyright"] = json!(value);
    }
    if kind == "SCHEDULED" {
        let date = config["app_store"]["earliest_release_date"]
            .as_str()
            .context("Scheduled release needs earliest_release_date")?;
        time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339)?;
        attributes["earliestReleaseDate"] = json!(date);
    }
    if state(version)? == "PREPARE_FOR_SUBMISSION" {
        api.request(
            "PATCH",
            &format!("/v1/appStoreVersions/{id}"),
            &[],
            Some(&json!({"data":{"type":"appStoreVersions","id":id,"attributes":attributes}})),
        )?;
        let actual = api.request("GET", &format!("/v1/appStoreVersions/{id}"), &[], None)?;
        ensure!(
            attributes
                .as_object()
                .unwrap()
                .iter()
                .all(|(k, v)| actual["data"]["attributes"][k] == *v),
            "Release policy readback differs"
        );
        if config["app_store"]["phased_release"] == true {
            let existing = api::optional(
                api,
                &format!("/v1/appStoreVersions/{id}/appStoreVersionPhasedRelease"),
                &[],
            )?;
            if existing.as_ref().is_none_or(|v| v["data"].is_null()) {
                api.request("POST","/v1/appStoreVersionPhasedReleases",&[],Some(&json!({"data":{"type":"appStoreVersionPhasedReleases","attributes":{"phasedReleaseState":"INACTIVE"},"relationships":{"appStoreVersion":{"data":{"type":"appStoreVersions","id":id}}}}})))?;
            }
            let actual = api.request(
                "GET",
                &format!("/v1/appStoreVersions/{id}/appStoreVersionPhasedRelease"),
                &[],
                None,
            )?;
            ensure!(
                actual["data"]["id"].as_str().is_some(),
                "Phased release configuration has not been confirmed"
            );
        }
    } else {
        ensure!(
            version["attributes"]["releaseType"] == kind,
            "Submitted version has a different release policy"
        );
    }
    Ok(())
}
pub fn publish(
    app: &App,
    release: &mut Release,
    api: &mut impl Api,
    confirmed: bool,
) -> Result<()> {
    ensure!(
        confirmed,
        "Pass --confirm to release the approved app publicly"
    );
    let version = version_record(app, release, api)?.context("App Store version missing")?;
    let id = api::id(&version)?;
    ensure!(
        release.receipt["version_id"] == id,
        "Release has no matching staged version receipt"
    );
    let build = processed(app, release, api)?.context("Build is not ready")?;
    let selected = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{id}/build"),
        &[],
        None,
    )?;
    ensure!(
        selected["data"]["id"] == build["id"],
        "Approved version contains another build"
    );
    let current = state(&version)?;
    if [
        "READY_FOR_SALE",
        "READY_FOR_DISTRIBUTION",
        "PROCESSING_FOR_APP_STORE",
        "PENDING_APPLE_RELEASE",
    ]
    .contains(&current)
    {
        return release.checkpoint(json!({"status":"release-requested","readback_state":current}));
    }
    ensure!(
        current == "PENDING_DEVELOPER_RELEASE" && version["attributes"]["releaseType"] == "MANUAL",
        "App must be approved and awaiting manual release"
    );
    ensure!(
        release.receipt["publish_intent"] != true,
        "A release request may already have been accepted; check store status before repeating it"
    );
    release.checkpoint(json!({"publish_intent":true}))?;
    let response=api.request("POST","/v1/appStoreVersionReleaseRequests",&[],Some(&json!({"data":{"type":"appStoreVersionReleaseRequests","relationships":{"appStoreVersion":{"data":{"type":"appStoreVersions","id":id}}}}})))?;
    release.checkpoint(
        json!({"publish_request_id":api::id(&response["data"])? ,"status":"release-requested"}),
    )
}
pub fn phased(app: &App, release: &mut Release, api: &mut impl Api, state: &str) -> Result<()> {
    ensure!(
        ["ACTIVE", "PAUSED", "COMPLETE"].contains(&state),
        "Phased state must be ACTIVE, PAUSED or COMPLETE"
    );
    let version = version_record(app, release, api)?.context("App Store version missing")?;
    let version_id = api::id(&version)?;
    ensure!(
        release.receipt["version_id"] == version_id,
        "Phased release belongs to another version"
    );
    let selected = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{version_id}/build"),
        &[],
        None,
    )?;
    ensure!(
        selected["data"]["id"] == release.receipt["build_id"],
        "Phased release contains another build"
    );
    let value = api.request(
        "GET",
        &format!("/v1/appStoreVersions/{version_id}/appStoreVersionPhasedRelease"),
        &[],
        None,
    )?;
    let id = api::id(&value["data"])?;
    api.request("PATCH",&format!("/v1/appStoreVersionPhasedReleases/{id}"),&[],Some(&json!({"data":{"type":"appStoreVersionPhasedReleases","id":id,"attributes":{"phasedReleaseState":state}}})))?;
    let actual = api.request(
        "GET",
        &format!("/v1/appStoreVersionPhasedReleases/{id}"),
        &[],
        None,
    )?;
    ensure!(
        actual["data"]["attributes"]["phasedReleaseState"] == state,
        "Phased release readback differs"
    );
    release.checkpoint(json!({"phased_state":state}))
}
