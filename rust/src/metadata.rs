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
    review_attributes(app)?;
    Ok(locales)
}
fn screenshot_files(app: &App, locale: &str) -> Result<Vec<std::path::PathBuf>> {
    let root = app.path("screenshots_path")?.join(locale);
    if !root.exists() {
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
        crate::images::store_png(&data).context("Screenshot PNG is damaged")?;
        let count = counts.entry(class).or_insert(0);
        *count += 1;
        ensure!(
            *count <= 10,
            "At most 10 screenshots per device class and locale are allowed"
        );
        files.push(file);
    }
    files.sort();
    Ok(files)
}
fn review_attributes(app: &App) -> Result<Option<serde_json::Value>> {
    let path = app.root.join("store/review.json");
    if !path.exists() {
        return Ok(None);
    }
    fsutil::confined(&app.root, &path)?;
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
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
        let password = std::env::var("IOS_RELEASE_DEMO_PASSWORD")
            .context("Set IOS_RELEASE_DEMO_PASSWORD privately for the review account")?;
        ensure!(!password.is_empty(), "Review demo password is empty");
        value["demoAccountPassword"] = json!(password);
    }
    Ok(Some(value))
}
fn review_details(app: &App, version: &str, api: &mut impl Api) -> Result<()> {
    let Some(attributes) = review_attributes(app)? else {
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
    let local = preflight(app)?;
    let version = release.receipt["version_id"]
        .as_str()
        .context("Run store stage before updating metadata")?
        .to_owned();
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
        screenshots(app, &locale, &localization_id, api)?;
        changed.push(locale);
    }
    ensure!(!changed.is_empty(), "No store metadata locales were found");
    review_details(app, &version, api)?;
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
pub fn screenshots(app: &App, locale: &str, localization: &str, api: &mut impl Api) -> Result<()> {
    let root = app.path("screenshots_path")?.join(locale);
    if !root.exists() {
        return Ok(());
    }
    let mut sets = api::list(
        api,
        &format!("/v1/appStoreVersionLocalizations/{localization}/appScreenshotSets"),
        &[],
    )?;
    let files = screenshot_files(app, locale)?;
    let mut per_class = std::collections::BTreeMap::<String, Vec<(String, String)>>::new();
    for file in files {
        fsutil::confined(&root, &file)?;
        let data = fs::read(&file)?;
        ensure!(
            data.len() >= 33 && data.starts_with(b"\x89PNG\r\n\x1a\n"),
            "Screenshot is not PNG"
        );
        ensure!(
            data[25] == 2,
            "Store screenshots must be RGB PNGs without alpha; use ios-release screenshots or export an opaque RGB PNG"
        );
        let width = u32::from_be_bytes(data[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(data[20..24].try_into().unwrap());
        let kind = display_type(width, height)?;
        let set = if let Some(set) = sets
            .iter()
            .find(|s| s["attributes"]["screenshotDisplayType"] == kind)
        {
            set.clone()
        } else {
            let set=api.request("POST","/v1/appScreenshotSets",&[],Some(&json!({"data":{"type":"appScreenshotSets","attributes":{"screenshotDisplayType":kind},"relationships":{"appStoreVersionLocalization":{"data":{"type":"appStoreVersionLocalizations","id":localization}}}}})))?["data"].clone();
            sets.push(set.clone());
            set
        };
        let set_id = api::id(&set)?.to_owned();
        let checksum = format!("{:x}", Md5::digest(&data));
        let name = format!(
            "ios-release-{}-{}",
            fsutil::sha256(&file)?,
            file.file_name().unwrap().to_string_lossy()
        );
        let existing = api::list(
            api,
            &format!("/v1/appScreenshotSets/{set_id}/appScreenshots"),
            &[],
        )?;
        let matching = existing
            .iter()
            .filter(|s| s["attributes"]["fileName"] == name)
            .collect::<Vec<_>>();
        ensure!(
            matching.len() <= 1,
            "Duplicate store screenshots named {name}; resolve them in App Store Connect"
        );
        let asset = if let Some(asset) = matching.first() {
            ensure!(
                asset["attributes"]["sourceFileChecksum"]
                    .as_str()
                    .is_none_or(|s| s == checksum),
                "Store screenshot {name} has different bytes; rename a replacement instead of silently deleting it"
            );
            (*asset).clone()
        } else {
            api.request("POST","/v1/appScreenshots",&[],Some(&json!({"data":{"type":"appScreenshots","attributes":{"fileName":name,"fileSize":data.len()},"relationships":{"appScreenshotSet":{"data":{"type":"appScreenshotSets","id":set_id}}}}})))?["data"].clone()
        };
        let id = api::id(&asset)?.to_owned();
        let state = asset["attributes"]["assetDeliveryState"]["state"]
            .as_str()
            .context("Missing screenshot asset state")?;
        if state != "COMPLETE" && state != "UPLOAD_COMPLETE" {
            ensure!(
                state == "AWAITING_UPLOAD",
                "Screenshot reservation failed or is still processing; resume metadata after Apple finishes"
            );
            api.transfer(&file, &asset["attributes"]["uploadOperations"])?;
            api.request("PATCH",&format!("/v1/appScreenshots/{id}"),&[],Some(&json!({"data":{"type":"appScreenshots","id":id,"attributes":{"uploaded":true,"sourceFileChecksum":checksum}}})))?;
        }
        let actual = api.request("GET", &format!("/v1/appScreenshots/{id}"), &[], None)?;
        ensure!(
            actual["data"]["attributes"]["sourceFileChecksum"] == checksum,
            "Screenshot checksum readback differs"
        );
        per_class.entry(set_id).or_default().push((id, name));
    }
    for (set, assets) in per_class {
        let existing = api::list(
            api,
            &format!("/v1/appScreenshotSets/{set}/appScreenshots"),
            &[],
        )?;
        let mut order = assets.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        for resource in existing {
            let id = api::id(&resource)?.to_owned();
            if !order.contains(&id) {
                order.push(id);
            }
        }
        ensure!(
            order.len() <= 10,
            "Apple accepts at most 10 screenshots per device class; remove superseded screenshots in App Store Connect before adding replacements"
        );
        api.request("PATCH",&format!("/v1/appScreenshotSets/{set}/relationships/appScreenshots"),&[],Some(&json!({"data":order.iter().map(|id|json!({"type":"appScreenshots","id":id})).collect::<Vec<_>>()})))?;
    }
    Ok(())
}
