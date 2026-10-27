//! App-owned accessibility declarations; no inferred feature claims.
use crate::{
    api::{self, Api},
    config::App,
    fsutil,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs};

pub const FEATURES: [&str; 9] = [
    "supportsAudioDescriptions",
    "supportsCaptions",
    "supportsDarkInterface",
    "supportsDifferentiateWithoutColorAlone",
    "supportsLargerText",
    "supportsReducedMotion",
    "supportsSufficientContrast",
    "supportsVoiceControl",
    "supportsVoiceover",
];

pub fn validate(app: &App) -> Result<Option<Value>> {
    if app.config["accessibility_path"].is_null() {
        return Ok(None);
    }
    let path = app.path("accessibility_path")?;
    fsutil::confined(&app.root, &path)?;
    let value: Value = serde_json::from_slice(&fs::read(path)?)?;
    let object = value
        .as_object()
        .context("Accessibility document must be an object")?;
    ensure!(
        object
            .keys()
            .all(|s| ["declarations", "accessibilityUrl"].contains(&s.as_str())),
        "Unsupported accessibility field"
    );
    if let Some(url) = value.get("accessibilityUrl") {
        let url = reqwest::Url::parse(url.as_str().context("Accessibility URL must be a string")?)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none(),
            "Accessibility URL must be public HTTPS"
        );
    }
    let declarations = value["declarations"]
        .as_array()
        .context("Accessibility declarations missing")?;
    ensure!(
        !declarations.is_empty(),
        "Accessibility declarations cannot be empty"
    );
    let mut seen = BTreeSet::new();
    for declaration in declarations {
        let family = declaration["deviceFamily"]
            .as_str()
            .context("Missing accessibility device family")?;
        ensure!(
            ["IPHONE", "IPAD"].contains(&family) && seen.insert(family),
            "Unsupported or duplicate accessibility device family"
        );
        let fields = declaration
            .as_object()
            .context("Invalid accessibility declaration")?;
        ensure!(
            fields
                .keys()
                .all(|s| s == "deviceFamily" || FEATURES.contains(&s.as_str())),
            "Unsupported accessibility feature"
        );
        ensure!(
            FEATURES.iter().all(|key| declaration[key].is_boolean()),
            "Every accessibility feature requires an explicit boolean"
        );
    }
    Ok(Some(value))
}

pub fn sync(app: &App, app_id: &str, api: &mut impl Api) -> Result<()> {
    let Some(value) = validate(app)? else {
        return Ok(());
    };
    let existing = api::list(
        api,
        &format!("/v1/apps/{app_id}/accessibilityDeclarations"),
        &[],
    )?;
    // Check every family before changing any declaration.
    let mut selected = Vec::new();
    for wanted in value["declarations"].as_array().unwrap() {
        let matches = existing
            .iter()
            .filter(|v| {
                v["attributes"]["deviceFamily"] == wanted["deviceFamily"]
                    && v["attributes"]["state"] != "REPLACED"
            })
            .collect::<Vec<_>>();
        let drafts = matches
            .iter()
            .copied()
            .filter(|v| v["attributes"]["state"] == "DRAFT")
            .collect::<Vec<_>>();
        ensure!(
            drafts.len() <= 1,
            "Ambiguous draft accessibility declaration"
        );
        let current = if !drafts.is_empty() {
            Some(drafts[0])
        } else {
            ensure!(
                matches.len() <= 1,
                "Ambiguous active accessibility declaration"
            );
            matches.first().copied()
        };
        if let Some(current) = current {
            let same = wanted
                .as_object()
                .unwrap()
                .iter()
                .all(|(k, v)| current["attributes"][k] == *v);
            ensure!(
                same || current["attributes"]["state"] == "DRAFT",
                "Create an accessibility draft in App Store Connect before changing published features"
            );
        }
        selected.push((wanted, current));
    }
    for (wanted, current) in selected {
        let id = if let Some(current) = current {
            let id = api::id(current)?.to_owned();
            if !wanted
                .as_object()
                .unwrap()
                .iter()
                .all(|(k, v)| current["attributes"][k] == *v)
            {
                let mut attributes = wanted.clone();
                attributes.as_object_mut().unwrap().remove("deviceFamily");
                api.request("PATCH", &format!("/v1/accessibilityDeclarations/{id}"), &[], Some(&json!({"data":{"type":"accessibilityDeclarations","id":id,"attributes":attributes}})))?;
            }
            id
        } else {
            let response = api.request("POST", "/v1/accessibilityDeclarations", &[], Some(&json!({"data":{"type":"accessibilityDeclarations","attributes":wanted,"relationships":{"app":{"data":{"type":"apps","id":app_id}}}}})))?;
            api::id(&response["data"])?.to_owned()
        };
        let actual = api.request(
            "GET",
            &format!("/v1/accessibilityDeclarations/{id}"),
            &[],
            None,
        )?;
        ensure!(
            ["DRAFT", "PUBLISHED"]
                .iter()
                .any(|state| actual["data"]["attributes"]["state"] == *state)
                && wanted
                    .as_object()
                    .unwrap()
                    .iter()
                    .all(|(key, v)| actual["data"]["attributes"][key] == *v),
            "Accessibility readback differs"
        );
    }
    if let Some(url) = value.get("accessibilityUrl") {
        api.request(
            "PATCH",
            &format!("/v1/apps/{app_id}"),
            &[],
            Some(
                &json!({"data":{"type":"apps","id":app_id,"attributes":{"accessibilityUrl":url}}}),
            ),
        )?;
        let actual = api.request("GET", &format!("/v1/apps/{app_id}"), &[], None)?;
        ensure!(
            actual["data"]["attributes"]["accessibilityUrl"] == *url,
            "Accessibility URL readback differs"
        );
    }
    Ok(())
}
