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

pub fn sync(app: &App, release: &mut Release, api: &mut impl Api) -> Result<()> {
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
            ["PREPARE_FOR_SUBMISSION", "READY_FOR_REVIEW"]
                .contains(&v["attributes"]["appStoreState"].as_str().unwrap_or(""))
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
    for folder in fs::read_dir(&root)? {
        let folder = folder?;
        if !folder.file_type()?.is_dir() {
            continue;
        }
        let locale = folder.file_name().to_string_lossy().into_owned();
        crate::config::filename(&locale)?;
        let mut attributes = json!({});
        for (file, key) in [
            ("description.txt", "description"),
            ("keywords.txt", "keywords"),
            ("support_url.txt", "supportUrl"),
            ("release_notes.txt", "whatsNew"),
            ("promotional_text.txt", "promotionalText"),
            ("marketing_url.txt", "marketingUrl"),
        ] {
            if let Some(value) = read(&folder.path(), file)? {
                attributes[key] = json!(value);
            }
        }
        for key in ["description", "supportUrl"] {
            ensure!(
                attributes[key].as_str().is_some_and(|s| !s.is_empty()),
                "Add {key} for {locale} before metadata delivery"
            );
        }
        attributes["locale"] = json!(locale);
        let matches = locales
            .iter()
            .filter(|l| l["attributes"]["locale"] == locale)
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "Ambiguous store locale");
        let resource = if let Some(existing) = matches.first() {
            let id = api::id(existing)?;
            api.request("PATCH",&format!("/v1/appStoreVersionLocalizations/{id}"),&[],Some(&json!({"data":{"type":"appStoreVersionLocalizations","id":id,"attributes":attributes}})))?["data"].clone()
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
        let mut app_attributes = json!({"locale":locale});
        for (file, key) in [
            ("name.txt", "name"),
            ("subtitle.txt", "subtitle"),
            ("privacy_url.txt", "privacyPolicyUrl"),
        ] {
            if let Some(value) = read(&folder.path(), file)? {
                app_attributes[key] = json!(value);
            }
        }
        ensure!(
            app_attributes["name"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "Set your App Store name for {locale}"
        );
        ensure!(
            app_attributes["privacyPolicyUrl"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "Set your privacy policy URL for {locale}"
        );
        let matches = info_locales
            .iter()
            .filter(|l| l["attributes"]["locale"] == locale)
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "Ambiguous app information locale");
        let resource = if let Some(existing) = matches.first() {
            let id = api::id(existing)?;
            api.request("PATCH",&format!("/v1/appInfoLocalizations/{id}"),&[],Some(&json!({"data":{"type":"appInfoLocalizations","id":id,"attributes":app_attributes}})))?["data"].clone()
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
pub fn display_type(width: u32, height: u32) -> Result<&'static str> {
    match (width.min(height), width.max(height)) {
        // Apple's API retains the APP_IPHONE_67 name for the current 6.9-inch class.
        (1320, 2868) | (1290, 2796) | (1260, 2736) => Ok("APP_IPHONE_67"),
        (1284, 2778) | (1242, 2688) => Ok("APP_IPHONE_65"),
        (1242, 2208) => Ok("APP_IPHONE_55"),
        (2064, 2752) | (2048, 2732) => Ok("APP_IPAD_PRO_3GEN_129"),
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
    let mut files = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|s| s == "png") {
            files.push(entry.path());
        }
    }
    files.sort();
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
