use crate::config::{App, relative_path};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::fs;

pub fn specifiers(text: &str) -> Option<Vec<(usize, String)>> {
    let regex = Regex::new(r"%(?:(\d+)\$)?(lld|ld|d|@|f|\.\d+f)").unwrap();
    let plain = text.replace("%%", "");
    let captures: Vec<_> = regex.captures_iter(&plain).collect();
    let positional = captures.iter().filter(|c| c.get(1).is_some()).count();
    if positional != 0 && positional != captures.len() {
        return None;
    }
    let mut result: Vec<_> = captures
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                c.get(1)
                    .and_then(|v| v.as_str().parse().ok())
                    .unwrap_or(i + 1),
                c[2].to_owned(),
            )
        })
        .collect();
    result.sort();
    Some(result)
}

pub fn audit(app: &App) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    for path in app.names("localization_catalogs")? {
        let path_on_disk = app.root.join(relative_path(&path)?);
        crate::fsutil::confined(&app.root, &path_on_disk)?;
        let catalog: Value = serde_json::from_slice(&fs::read(path_on_disk)?)?;
        errors.extend(
            catalog_errors(&catalog, &app.names("localization_locales")?)?
                .into_iter()
                .map(|error| format!("{path}: {error}")),
        );
    }
    Ok(errors)
}

pub fn catalog_errors(catalog: &Value, locales: &[String]) -> Result<Vec<String>> {
    let token = Regex::new(r"\$\{[A-Za-z]+\}")?;
    let inflect = Regex::new(r"\^\[(.*?)\]\(inflect: true\)")?;
    let source = catalog["sourceLanguage"].as_str().unwrap_or("en");
    let mut errors = Vec::new();
    for (key, entry) in catalog["strings"]
        .as_object()
        .context("Missing catalog strings")?
    {
        if entry["shouldTranslate"] == false {
            continue;
        }
        let english = entry["localizations"][source]["stringUnit"]["value"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(key);
        let plain = inflect.replace_all(english, "$1");
        for locale in locales {
            let Some(localization) = entry["localizations"].get(locale) else {
                errors.push(format!("{key}: no {locale} translation"));
                continue;
            };
            let mut units = Vec::new();
            if let Some(unit) = localization.get("stringUnit") {
                units.push((None, false, unit));
            }
            let mut groups = Vec::new();
            if let Some(group) = localization["variations"].get("plural") {
                groups.push((false, group));
            }
            if let Some(substitutions) = localization["substitutions"].as_object() {
                for substitution in substitutions.values() {
                    if let Some(group) = substitution["variations"].get("plural") {
                        groups.push((true, group));
                    }
                }
            }
            for (fragment, group) in groups {
                let group = group.as_object().context("Malformed plural group")?;
                let needed: &[&str] = match locale.as_str() {
                    "ar" => &["zero", "one", "two", "few", "many", "other"],
                    "pl" => &["one", "few", "many", "other"],
                    "ja" | "ko" | "zh-Hans" | "zh-Hant" => &["other"],
                    _ => &["one", "other"],
                };
                for category in needed {
                    if !group.contains_key(*category) {
                        errors.push(format!("{key}: {locale} plural lacks {category}"));
                    }
                }
                for (category, node) in group {
                    units.push((
                        Some(category.as_str()),
                        fragment,
                        node.get("stringUnit")
                            .context("Missing plural string unit")?,
                    ));
                }
            }
            for (category, fragment, unit) in units {
                let value = unit["value"].as_str().unwrap_or("");
                let label = format!("{key} [{locale}/{}]", category.unwrap_or(""));
                if unit["state"] != "translated" {
                    errors.push(format!("{label}: state is not translated"));
                }
                if value.trim().is_empty() {
                    errors.push(format!("{label}: empty"));
                }
                if value.contains("inflect: true") {
                    errors.push(format!("{label}: inflect markup only works in English"));
                }
                if fragment || value.contains("%#@") {
                    continue;
                }
                let tokens = |s: &str| {
                    let mut values: Vec<_> =
                        token.find_iter(s).map(|v| v.as_str().to_owned()).collect();
                    values.sort();
                    values
                };
                if tokens(value) != tokens(english) {
                    errors.push(format!("{label}: token mismatch"));
                }
                if value.matches("**").count() != english.matches("**").count() {
                    errors.push(format!("{label}: Markdown mismatch"));
                }
                let got = specifiers(value);
                let spelled = locale == "ar"
                    && matches!(category, Some("zero" | "one" | "two"))
                    && got == Some(vec![]);
                if got.is_none() || (got != specifiers(&plain) && !spelled) {
                    errors.push(format!("{label}: format specifier mismatch"));
                }
            }
        }
    }
    Ok(errors)
}
