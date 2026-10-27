use crate::{
    api::{self, Api},
    config::App,
    fsutil,
    store::Release,
};
use anyhow::{Context, Result, ensure};
use md5::{Digest, Md5};
use serde_json::json;
use std::{fs, path::Path};

type LocaleMetadata = (String, serde_json::Value, serde_json::Value);
pub fn preflight(app: &App) -> Result<Vec<LocaleMetadata>> {
    ensure!(
        ["preserve", "replace-configured"].contains(
            &app.config["app_store"]["screenshot_policy"]
                .as_str()
                .unwrap_or("preserve")
        ),
        "Unknown screenshot policy"
    );
    let root = app.path("metadata_path")?;
    ensure!(root.is_dir(), "Fill store/metadata before delivery");
    let mut locales = vec![];
    for folder in fs::read_dir(&root)? {
        let folder = folder?;
        if !folder.file_type()?.is_dir() {
            continue;
        }
        fsutil::confined(&root, &folder.path())?;
        let locale = folder.file_name().to_string_lossy().into_owned();
        if locale == "review_information" {
            continue;
        }
        crate::config::filename(&locale)?;
        let mut version = json!({"locale":locale});
        let mut info = json!({"locale":locale});
        for (file, key, limit, app_info) in [
            ("description.txt", "description", 4000, false),
            ("keywords.txt", "keywords", 100, false),
            ("support_url.txt", "supportUrl", 1000, false),
            ("release_notes.txt", "whatsNew", 4000, false),
            ("promotional_text.txt", "promotionalText", 170, false),
            ("marketing_url.txt", "marketingUrl", 1000, false),
            ("name.txt", "name", 30, true),
            ("subtitle.txt", "subtitle", 30, true),
            ("privacy_url.txt", "privacyPolicyUrl", 1000, true),
        ] {
            if let Some(value) = read(&folder.path(), file)? {
                ensure!(
                    value.chars().count() <= limit,
                    "{locale}/{file} exceeds Apple's {limit} character limit"
                );
                ensure!(
                    value != "Describe what your app does.",
                    "Replace the starter description in {locale}/{file}"
                );
                if key.ends_with("Url") && !value.is_empty() {
                    let url = reqwest::Url::parse(&value)
                        .context("Store URL must be a complete HTTPS URL")?;
                    ensure!(
                        url.scheme() == "https"
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none(),
                        "Use a public HTTPS URL without credentials for {locale}/{file}"
                    );
                }
                if app_info {
                    info[key] = json!(value);
                } else {
                    version[key] = json!(value);
                }
            }
        }
        for (value, key) in [
            (&version, "description"),
            (&version, "supportUrl"),
            (&info, "name"),
            (&info, "privacyPolicyUrl"),
        ] {
            ensure!(
                value[key].as_str().is_some_and(|s| !s.is_empty()),
                "Fill {key} for {locale} before metadata delivery"
            );
        }
        screenshot_files(app, &locale)?;
        locales.push((locale, version, info));
    }
    ensure!(!locales.is_empty(), "No store metadata locales were found");
    locales.sort_by(|a, b| a.0.cmp(&b.0));
    let expected = app
        .names("locales")?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let found = locales
        .iter()
        .map(|v| v.0.clone())
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(
        expected == found,
        "Store locales differ from the configured inventory; expected {expected:?}, found {found:?}"
    );
    crate::accessibility::validate(app)?;
    review_attributes(app, false)?;
    Ok(locales)
}
pub fn screenshot_files(app: &App, locale: &str) -> Result<Vec<std::path::PathBuf>> {
    let root = app.path("screenshots_path")?.join(locale);
    if !root.exists() {
        ensure!(
            app.names("screens")?.is_empty(),
            "Missing screenshot directory for {locale}"
        );
        return Ok(vec![]);
    }
    fsutil::confined(&app.root, &root)?;
    let mut files = vec![];
    let mut counts = std::collections::BTreeMap::new();
    for entry in fs::read_dir(&root)? {
        let file = entry?.path();
        if !file.extension().is_some_and(|s| s == "png") {
            continue;
        }
        fsutil::confined(&root, &file)?;
        ensure!(
            fs::metadata(&file)?.len() <= 32 * 1024 * 1024,
            "Screenshot is too large"
        );
        let data = fs::read(&file)?;
        ensure!(
            data.len() >= 33 && data.starts_with(b"\x89PNG\r\n\x1a\n") && data[25] == 2,
            "Use an opaque RGB PNG for {}",
            file.display()
        );
        let width = u32::from_be_bytes(data[16..20].try_into()?);
        let height = u32::from_be_bytes(data[20..24].try_into()?);
        let class = display_type(width, height)?;
        crate::images::validate_store_png(&data).context("Screenshot PNG is damaged")?;
        let count = counts.entry(class).or_insert(0);
        *count += 1;
        ensure!(
            *count <= 10,
            "At most 10 screenshots per device class and locale are allowed"
        );
        files.push(file);
    }
    files.sort();
    let screens = app.names("screens")?;
    if !screens.is_empty() {
        let classes = app.config["screenshot_classes"]
            .as_object()
            .context("Configure screenshot_classes for required screenshots")?;
        ensure!(!classes.is_empty(), "Screenshot classes cannot be empty");
        let mut expected_files = std::collections::BTreeSet::new();
        for (class, settings) in classes {
            let names = crate::config::strings(&settings["names"])?;
            for screen in &screens {
                let matching = files
                    .iter()
                    .filter(|file| {
                        let name = file.file_name().unwrap().to_string_lossy();
                        names
                            .iter()
                            .any(|device| name == format!("{device}-{screen}.png"))
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    matching.len() == 1,
                    "{locale}: expected exactly one {class}/{screen} screenshot"
                );
                let data = fs::read(matching[0])?;
                let size = json!([
                    u32::from_be_bytes(data[16..20].try_into()?),
                    u32::from_be_bytes(data[20..24].try_into()?)
                ]);
                ensure!(
                    settings["sizes"]
                        .as_array()
                        .is_some_and(|v| v.contains(&size)),
                    "Screenshot dimensions differ from configured class {class}"
                );
                expected_files.insert(matching[0].to_path_buf());
            }
        }
        ensure!(
            expected_files.len() == files.len(),
            "{locale}: unexpected screenshot files"
        );
    }
    Ok(files)
}
pub fn review_document(app: &App) -> Result<Option<serde_json::Value>> {
    let path = app.root.join("store/review.json");
    if path.exists() {
        fsutil::confined(&app.root, &path)?;
        return Ok(Some(serde_json::from_slice(&fs::read(path)?)?));
    }
    let folder = app.path("metadata_path")?.join("review_information");
    if !folder.exists() {
        return Ok(None);
    }
    ensure!(
        read(&folder, "demo_password.txt")?
            .unwrap_or_default()
            .is_empty(),
        "Move the review password out of tracked metadata into IOS_RELEASE_DEMO_PASSWORD"
    );
    let mut value = json!({});
    for (file, key) in [
        ("first_name.txt", "contactFirstName"),
        ("last_name.txt", "contactLastName"),
        ("phone_number.txt", "contactPhone"),
        ("email_address.txt", "contactEmail"),
        ("notes.txt", "notes"),
        ("demo_user.txt", "demoAccountName"),
    ] {
        if let Some(text) = read(&folder, file)? {
            value[key] = json!(text);
        }
    }
    value["demoAccountRequired"] = json!(
        value["demoAccountName"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    Ok(Some(value))
}
fn review_attributes(app: &App, private: bool) -> Result<Option<serde_json::Value>> {
    let Some(mut value) = review_document(app)? else {
        return Ok(None);
    };
    let allowed = [
        "contactFirstName",
        "contactLastName",
        "contactPhone",
        "contactEmail",
        "demoAccountRequired",
        "demoAccountName",
        "notes",
    ];
    ensure!(
        value
            .as_object()
            .is_some_and(|v| v.keys().all(|k| allowed.contains(&k.as_str()))),
        "Review details contain unsupported fields; supply a demo password with IOS_RELEASE_DEMO_PASSWORD"
    );
    for key in [
        "contactFirstName",
        "contactLastName",
        "contactPhone",
        "contactEmail",
    ] {
        ensure!(
            value[key]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 1000),
            "Fill {key} in store/review.json"
        );
    }
    ensure!(
        value["contactEmail"].as_str().unwrap().contains('@'),
        "Review contact email is invalid"
    );
    let required = value["demoAccountRequired"]
        .as_bool()
        .context("Declare demoAccountRequired in store/review.json")?;
    if required {
        ensure!(
            value["demoAccountName"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "Review needs demoAccountName"
        );
        if private {
            let password = std::env::var("IOS_RELEASE_DEMO_PASSWORD")
                .context("Set IOS_RELEASE_DEMO_PASSWORD privately for the review account")?;
            ensure!(!password.is_empty(), "Review demo password is empty");
            value["demoAccountPassword"] = json!(password);
        }
    }
    Ok(Some(value))
}
fn review_details(app: &App, version: &str, api: &mut impl Api) -> Result<()> {
    let Some(attributes) = review_attributes(app, true)? else {
        return Ok(());
    };
    let existing = api::optional(
        api,
        &format!("/v1/appStoreVersions/{version}/appStoreReviewDetail"),
        &[],
    )?;
    let resource = if let Some(existing) = existing.filter(|v| !v["data"].is_null()) {
        let id = api::id(&existing["data"])?;
        api.request(
            "PATCH",
            &format!("/v1/appStoreReviewDetails/{id}"),
            &[],
            Some(&json!({"data":{"type":"appStoreReviewDetails","id":id,"attributes":attributes}})),
        )?["data"]
            .clone()
    } else {
        api.request("POST", "/v1/appStoreReviewDetails", &[], Some(&json!({"data":{"type":"appStoreReviewDetails","attributes":attributes,"relationships":{"appStoreVersion":{"data":{"type":"appStoreVersions","id":version}}}}})))?["data"].clone()
    };
    let id = api::id(&resource)?;
    let actual = api.request("GET", &format!("/v1/appStoreReviewDetails/{id}"), &[], None)?;
    ensure!(
        attributes
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.as_str() != "demoAccountPassword")
            .all(|(k, v)| actual["data"]["attributes"][k] == *v),
        "Review contact readback differs"
    );
    Ok(())
}

pub fn sync(app: &App, release: &mut Release, api: &mut impl Api) -> Result<()> {
    // New preparations deliver their immutable content, even from a newer checkout.
    let sealed;
    let app = if release.manifest["schema_version"] == 3 {
        crate::content::verify(&release.directory, &release.manifest)?;
        sealed = App::load(&release.directory, Path::new("app.json"))?;
        &sealed
    } else {
        app
    };
    let local = preflight(app)?;
    let version = crate::store::editable_stage(app, release, api)?;
    let locales = api::list(
        api,
        &format!("/v1/appStoreVersions/{version}/appStoreVersionLocalizations"),
        &[],
    )?;
    let root = app.path("metadata_path")?;
    ensure!(
        root.is_dir(),
        "Fill store/metadata with your app description, support URL, privacy URL and release notes"
    );
    let app_id = crate::store::app_id(app, api)?;
    let infos = api::list(api, &format!("/v1/apps/{app_id}/appInfos"), &[])?;
    let editable = infos
        .iter()
        .filter(|v| {
            ["PREPARE_FOR_SUBMISSION", "READY_FOR_REVIEW"].contains(
                &v["attributes"]["state"]
                    .as_str()
                    .or(v["attributes"]["appStoreState"].as_str())
                    .unwrap_or(""),
            )
        })
        .collect::<Vec<_>>();
    ensure!(
        editable.len() == 1,
        "Apple has no unique editable app information record"
    );
    let info_id = api::id(editable[0])?.to_owned();
    let info_locales = api::list(
        api,
        &format!("/v1/appInfos/{info_id}/appInfoLocalizations"),
        &[],
    )?;
    let mut changed = Vec::new();
    for (locale, attributes, app_attributes) in local {
        let matches = locales
            .iter()
            .filter(|l| l["attributes"]["locale"] == locale)
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "Ambiguous store locale");
        let resource = if let Some(existing) = matches.first() {
            let id = api::id(existing)?;
            api.request("PATCH",&format!("/v1/appStoreVersionLocalizations/{id}"),&[],Some(&json!({"data":{"type":"appStoreVersionLocalizations","id":id,"attributes":update_attributes(&attributes)}})))?["data"].clone()
        } else {
            api.request("POST","/v1/appStoreVersionLocalizations",&[],Some(&json!({"data":{"type":"appStoreVersionLocalizations","attributes":attributes,"relationships":{"appStoreVersion":{"data":{"type":"appStoreVersions","id":version}}}}})))?["data"].clone()
        };
        let localization_id = api::id(&resource)?.to_owned();
        let actual = api.request(
            "GET",
            &format!("/v1/appStoreVersionLocalizations/{localization_id}"),
            &[],
            None,
        )?;
        ensure!(
            attributes
                .as_object()
                .unwrap()
                .iter()
                .all(|(k, v)| actual["data"]["attributes"][k] == *v),
            "Store metadata readback differs for {locale}"
        );
        let matches = info_locales
            .iter()
            .filter(|l| l["attributes"]["locale"] == locale)
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "Ambiguous app information locale");
        let resource = if let Some(existing) = matches.first() {
            let id = api::id(existing)?;
            api.request("PATCH",&format!("/v1/appInfoLocalizations/{id}"),&[],Some(&json!({"data":{"type":"appInfoLocalizations","id":id,"attributes":update_attributes(&app_attributes)}})))?["data"].clone()
        } else {
            api.request("POST","/v1/appInfoLocalizations",&[],Some(&json!({"data":{"type":"appInfoLocalizations","attributes":app_attributes,"relationships":{"appInfo":{"data":{"type":"appInfos","id":info_id}}}}})))?["data"].clone()
        };
        let actual = api.request(
            "GET",
            &format!("/v1/appInfoLocalizations/{}", api::id(&resource)?),
            &[],
            None,
        )?;
        ensure!(
            app_attributes
                .as_object()
                .unwrap()
                .iter()
                .all(|(k, v)| actual["data"]["attributes"][k] == *v),
            "App metadata readback differs for {locale}"
        );
        screenshots(app, &locale, &localization_id, release, api)?;
        changed.push(locale);
    }
    ensure!(!changed.is_empty(), "No store metadata locales were found");
    review_details(app, &version, api)?;
    crate::accessibility::sync(app, &app_id, api)?;
    release.checkpoint(json!({"metadata_locales":changed,"status":"metadata-updated"}))
}
fn read(folder: &Path, name: &str) -> Result<Option<String>> {
    let path = folder.join(name);
    if !path.exists() {
        return Ok(None);
    }
    fsutil::confined(folder, &path)?;
    let value = fs::read_to_string(path)?;
    ensure!(value.len() < 100000, "Metadata field is too large");
    Ok(Some(value.trim().to_owned()))
}
pub fn update_attributes(value: &serde_json::Value) -> serde_json::Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("locale");
    }
    value
}
pub fn display_type(width: u32, height: u32) -> Result<&'static str> {
    match (width.min(height), width.max(height)) {
        // Apple's API retains the APP_IPHONE_67 name for the current 6.9-inch class.
        (1320, 2868) | (1290, 2796) | (1260, 2736) => Ok("APP_IPHONE_67"),
        (1284, 2778) | (1242, 2688) => Ok("APP_IPHONE_65"),
        (1242, 2208) => Ok("APP_IPHONE_55"),
        (1179, 2556) | (1206, 2622) => Ok("APP_IPHONE_61"),
        (1170, 2532) => Ok("APP_IPHONE_61"),
        (1125, 2436) | (1080, 2340) => Ok("APP_IPHONE_58"),
        (750, 1334) => Ok("APP_IPHONE_47"),
        (2064, 2752) | (2048, 2732) => Ok("APP_IPAD_PRO_3GEN_129"),
        (1488, 2266) | (1668, 2420) | (1668, 2388) | (1640, 2360) => Ok("APP_IPAD_PRO_3GEN_11"),
        (1668, 2224) => Ok("APP_IPAD_105"),
        (1536, 2048) => Ok("APP_IPAD_97"),
        _ => anyhow::bail!(
            "Unsupported App Store screenshot dimensions {width}x{height}; use a configured App Store device class"
        ),
    }
}
pub fn screenshots(
    app: &App,
    locale: &str,
    localization: &str,
    release: &mut Release,
    api: &mut impl Api,
) -> Result<()> {
    let files = screenshot_files(app, locale)?;
    if files.is_empty() {
        return Ok(());
    }
    let policy = app.config["app_store"]["screenshot_policy"]
        .as_str()
        .unwrap_or("preserve");
    ensure!(
        ["preserve", "replace-configured"].contains(&policy),
        "Unknown screenshot policy"
    );
    let replace = policy == "replace-configured";
    let mut grouped = std::collections::BTreeMap::<String, Vec<_>>::new();
    for file in files {
        let data = fs::read(&file)?;
        let kind = display_type(
            u32::from_be_bytes(data[16..20].try_into()?),
            u32::from_be_bytes(data[20..24].try_into()?),
        )?;
        let checksum = format!("{:x}", Md5::digest(&data));
        let name = format!(
            "ios-release-{}-{}",
            fsutil::sha256(&file)?,
            file.file_name().unwrap().to_string_lossy()
        );
        grouped
            .entry(kind.into())
            .or_default()
            .push((file, checksum, name, data.len()));
    }
    let sets = api::list(
        api,
        &format!("/v1/appStoreVersionLocalizations/{localization}/appScreenshotSets"),
        &[],
    )?;
    for (kind, files) in grouped {
        let matching = sets
            .iter()
            .filter(|s| s["attributes"]["screenshotDisplayType"] == kind)
            .collect::<Vec<_>>();
        ensure!(matching.len() <= 1, "Ambiguous screenshot display class");
        let set = if let Some(set) = matching.first() {
            (*set).clone()
        } else {
            api.request("POST", "/v1/appScreenshotSets", &[], Some(&json!({"data":{"type":"appScreenshotSets","attributes":{"screenshotDisplayType":kind},"relationships":{"appStoreVersionLocalization":{"data":{"type":"appStoreVersionLocalizations","id":localization}}}}})))?["data"].clone()
        };
        let set_id = api::id(&set)?.to_owned();
        let existing = api::list(
            api,
            &format!("/v1/appScreenshotSets/{set_id}/appScreenshots"),
            &[],
        )?;
        let mut used = std::collections::BTreeSet::new();
        let mut selected = Vec::new();
        for (_, checksum, name, _) in &files {
            // Reuse Fastlane assets by their bytes, regardless of their old filename.
            let asset = existing
                .iter()
                .find(|s| {
                    !used.contains(s["id"].as_str().unwrap_or(""))
                        && s["attributes"]["sourceFileChecksum"] == *checksum
                        && s["attributes"]["assetDeliveryState"]["state"] == "COMPLETE"
                })
                .or_else(|| {
                    existing.iter().find(|s| {
                        !used.contains(s["id"].as_str().unwrap_or(""))
                            && s["attributes"]["fileName"] == *name
                    })
                });
            if let Some(asset) = asset {
                ensure!(
                    asset["attributes"]["sourceFileChecksum"]
                        .as_str()
                        .is_none_or(|v| v == checksum),
                    "Existing screenshot checksum differs"
                );
                used.insert(api::id(asset)?.to_owned());
            }
            selected.push(asset.cloned());
        }
        let superseded = existing
            .iter()
            .filter(|s| !used.contains(s["id"].as_str().unwrap_or("")))
            .collect::<Vec<_>>();
        ensure!(
            replace || files.len() + superseded.len() <= 10,
            "Screenshot replacements exceed Apple's limit; explicitly configure app_store.screenshot_policy=replace-configured to replace this locale and class"
        );
        if replace && !superseded.is_empty() {
            // Persist the before-state and intended bytes BEFORE any deletion. Retrying resumes by checksum.
            let mut journal = release.receipt["screenshot_changes"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            journal.entry(set_id.clone()).or_insert_with(|| json!({"locale":locale,"display_type":kind,"before":existing,"desired":files.iter().map(|(_, checksum, name, size)|json!({"fileName":name,"sourceFileChecksum":checksum,"fileSize":size})).collect::<Vec<_>>()}));
            release.checkpoint(
                json!({"screenshot_changes":journal,"status":"screenshots-updating"}),
            )?;
            for old in superseded {
                api.request(
                    "DELETE",
                    &format!("/v1/appScreenshots/{}", api::id(old)?),
                    &[],
                    None,
                )?;
            }
        }
        let mut order = Vec::new();
        for ((file, checksum, name, size), existing) in files.iter().zip(selected) {
            let asset = if let Some(asset) = existing {
                asset
            } else {
                api.request("POST", "/v1/appScreenshots", &[], Some(&json!({"data":{"type":"appScreenshots","attributes":{"fileName":name,"fileSize":size},"relationships":{"appScreenshotSet":{"data":{"type":"appScreenshotSets","id":set_id}}}}})))?["data"].clone()
            };
            let id = api::id(&asset)?.to_owned();
            let state = asset["attributes"]["assetDeliveryState"]["state"]
                .as_str()
                .context("Missing screenshot asset state")?;
            if state != "COMPLETE" && state != "UPLOAD_COMPLETE" {
                ensure!(
                    state == "AWAITING_UPLOAD",
                    "Screenshot is not resumable; retry after Apple processing completes"
                );
                api.transfer(file, &asset["attributes"]["uploadOperations"])?;
                api.request("PATCH", &format!("/v1/appScreenshots/{id}"), &[], Some(&json!({"data":{"type":"appScreenshots","id":id,"attributes":{"uploaded":true,"sourceFileChecksum":checksum}}})))?;
            }
            let actual = api.request("GET", &format!("/v1/appScreenshots/{id}"), &[], None)?;
            ensure!(
                actual["data"]["attributes"]["sourceFileChecksum"] == *checksum
                    && actual["data"]["attributes"]["assetDeliveryState"]["state"] == "COMPLETE",
                "Screenshot not yet complete or checksum differs; resume metadata"
            );
            order.push(id);
        }
        if !replace {
            for old in existing {
                let id = api::id(&old)?.to_owned();
                if !order.contains(&id) {
                    order.push(id);
                }
            }
        }
        api.request("PATCH", &format!("/v1/appScreenshotSets/{set_id}/relationships/appScreenshots"), &[], Some(&json!({"data":order.iter().map(|id|json!({"type":"appScreenshots","id":id})).collect::<Vec<_>>()})))?;
        let readback = api::list(
            api,
            &format!("/v1/appScreenshotSets/{set_id}/appScreenshots"),
            &[],
        )?;
        ensure!(
            readback.iter().map(api::id).collect::<Result<Vec<_>>>()?
                == order.iter().map(String::as_str).collect::<Vec<_>>(),
            "Screenshot order readback differs"
        );
    }
    Ok(())
}
