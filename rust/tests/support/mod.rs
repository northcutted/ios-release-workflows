use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::sync::OnceLock;

/// Fixtures must honor Apple's schema, even when a permissive fake server would accept a request.
pub fn validate(
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: Option<&Value>,
) -> Result<()> {
    static CONTRACTS: OnceLock<Value> = OnceLock::new();
    let contracts = CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/apple-contracts.json")).unwrap()
    });
    let (template, methods) = contracts["paths"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(template, _)| {
            let expected = template.split('/').collect::<Vec<_>>();
            let actual = path.split('/').collect::<Vec<_>>();
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(a, b)| *a == "{}" || *a == b)
        })
        .with_context(|| format!("Endpoint absent from Apple contract: {method} {path}"))?;
    let operation = methods
        .get(method)
        .with_context(|| format!("Unsupported Apple method: {method} {template}"))?;
    let allowed = operation["query"].as_array().unwrap();
    for (name, _) in query {
        ensure!(
            allowed.iter().any(|v| v == name),
            "Unsupported Apple query {name} on {template}"
        );
    }
    for name in operation["required_query"].as_array().unwrap() {
        ensure!(
            query.iter().any(|(key, _)| name == key),
            "Missing required Apple query {name}"
        );
    }
    if let Some(schema) = operation.get("body") {
        validate_value(
            body.context("Apple operation requires a request body")?,
            schema,
        )?;
    } else {
        ensure!(body.is_none(), "Apple operation does not accept a body");
    }
    Ok(())
}
fn validate_value(value: &Value, schema: &Value) -> Result<()> {
    if value.is_null() && schema["nullable"] == true {
        return Ok(());
    }
    if let Some(values) = schema["enum"].as_array() {
        ensure!(
            values.contains(value),
            "Value outside Apple's enum: {value}"
        );
    }
    match schema["type"].as_str() {
        Some("object") => {
            let fields = value.as_object().context("Apple expects an object")?;
            let properties = schema["properties"]
                .as_object()
                .context("Object properties absent from contract")?;
            for name in schema["required"].as_array().into_iter().flatten() {
                ensure!(
                    name.as_str().is_some_and(|s| fields.contains_key(s)),
                    "Required Apple field missing: {name}"
                );
            }
            for (name, value) in fields {
                let property = properties
                    .get(name)
                    .with_context(|| format!("Unsupported Apple request field {name}"))?;
                validate_value(value, property)
                    .with_context(|| format!("Apple request field {name}"))?;
            }
        }
        Some("array") => {
            for value in value.as_array().context("Apple expects an array")? {
                validate_value(value, &schema["items"])?;
            }
        }
        Some("string") => ensure!(value.is_string(), "Apple expects a string"),
        Some("boolean") => ensure!(value.is_boolean(), "Apple expects a boolean"),
        Some("integer") => ensure!(
            value.as_i64().is_some() || value.as_u64().is_some(),
            "Apple expects an integer"
        ),
        _ => {}
    }
    Ok(())
}
