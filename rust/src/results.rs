use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde_json::Value;

fn cases<'a>(
    nodes: &'a Value,
    parents: Vec<String>,
    result: &mut Vec<(&'a Value, Vec<String>)>,
) -> Result<()> {
    for node in nodes
        .as_array()
        .context("Missing or malformed XCTest nodes")?
    {
        ensure!(node.is_object(), "Malformed XCTest node");
        let kind = node["nodeType"]
            .as_str()
            .context("Missing XCTest node type")?;
        if kind == "Test Case" {
            result.push((node, parents.clone()));
        } else if let Some(children) = node.get("children") {
            let mut parents = parents.clone();
            parents.push(
                node["name"]
                    .as_str()
                    .context("Missing test suite name")?
                    .to_owned(),
            );
            cases(children, parents, result)?;
        }
    }
    Ok(())
}

pub struct Report {
    pub xml: String,
    pub passed: bool,
    pub executed: usize,
}

pub fn junit(summary: &Value, tests: &Value) -> Result<Report> {
    let mut found = Vec::new();
    cases(&tests["testNodes"], Vec::new(), &mut found)?;
    ensure!(!found.is_empty(), "No XCTest cases executed");
    let states = [
        ("Passed", "passedTests"),
        ("Failed", "failedTests"),
        ("Skipped", "skippedTests"),
        ("Expected Failure", "expectedFailures"),
    ];
    for (state, key) in states {
        ensure!(
            summary[key].as_u64()
                == Some(found.iter().filter(|(n, _)| n["result"] == state).count() as u64),
            "XCTest summary and cases disagree: {key}"
        );
    }
    ensure!(
        summary["totalTestCount"].as_u64() == Some(found.len() as u64),
        "XCTest total count disagrees"
    );
    ensure!(
        states.iter().any(|(state, _)| summary["result"] == *state),
        "Unknown XCTest summary result"
    );
    let mut xml = format!(
        "<testsuites><testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\">",
        escape(summary["title"].as_str().unwrap_or("XCTest")),
        found.len(),
        summary["failedTests"],
        summary["skippedTests"].as_u64().unwrap() + summary["expectedFailures"].as_u64().unwrap()
    );
    for (node, parents) in &found {
        let state = node["result"]
            .as_str()
            .context("Missing XCTest case result")?;
        ensure!(
            states.iter().any(|(allowed, _)| state == *allowed),
            "Unknown XCTest case result"
        );
        let name = node["nodeIdentifier"]
            .as_str()
            .or(node["name"].as_str())
            .context("Missing XCTest case identity")?;
        let duration = match node.get("durationInSeconds") {
            Some(value) => value.as_f64().context("Invalid XCTest case duration")?,
            None => 0.0,
        };
        ensure!(
            duration.is_finite() && duration >= 0.0,
            "Invalid XCTest case duration"
        );
        xml.push_str(&format!(
            "<testcase name=\"{}\" classname=\"{}\" time=\"{duration}\">",
            escape(name),
            escape(&parents.join("/"))
        ));
        if state == "Failed" {
            let mut messages = summary["testFailures"]
                .as_array()
                .map(|failures| {
                    failures
                        .iter()
                        .filter(|f| f["testIdentifierString"] == name)
                        .filter_map(|f| f["failureText"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if messages.is_empty() {
                messages =
                    serde_json::to_string(node.get("children").unwrap_or(&serde_json::json!([])))?;
            }
            xml.push_str(&format!(
                "<failure message=\"XCTest failure\">{}</failure>",
                escape(&messages)
            ));
        } else if state == "Skipped" || state == "Expected Failure" {
            xml.push_str(&format!("<skipped message=\"{state}\"/>"));
        }
        xml.push_str("</testcase>");
    }
    xml.push_str("</testsuite></testsuites>\n");
    Ok(Report {
        xml,
        passed: summary["result"] == "Passed"
            && summary["failedTests"] == 0
            && summary["passedTests"].as_u64().unwrap() > 0,
        executed: found.len(),
    })
}

fn escape(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == '\t'
                || c == '\n'
                || c == '\r'
                || (c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}')
            {
                c
            } else {
                '\u{fffd}'
            }
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn bootstrap_failure(summary: &Value, tests: &Value) -> bool {
    fn assess(summary: &Value, tests: &Value) -> Result<()> {
        let report = junit(summary, tests)?;
        ensure!(
            !report.passed && summary["result"] == "Failed",
            "Not a failed runner"
        );
        for key in ["passedTests", "skippedTests", "expectedFailures"] {
            ensure!(
                summary[key].as_u64() == Some(0),
                "Application tests executed"
            );
        }
        let failures = summary["testFailures"]
            .as_array()
            .context("Missing bootstrap failures")?;
        ensure!(
            !failures.is_empty()
                && failures.len() == report.executed
                && summary["failedTests"].as_u64() == Some(report.executed as u64),
            "Inconsistent bootstrap evidence"
        );
        let mut found = Vec::new();
        cases(&tests["testNodes"], Vec::new(), &mut found)?;
        let runner = Regex::new(r"^\S+-Runner \(\d+\) encountered an error$")?;
        let mut identities = std::collections::HashSet::new();
        for failure in failures {
            let id = failure["testIdentifierString"]
                .as_str()
                .context("Missing failure ID")?;
            ensure!(
                runner.is_match(id) && identities.insert(id),
                "Not a unique runner bootstrap failure"
            );
            let text = failure["failureText"]
                .as_str()
                .context("Missing failure text")?;
            let stalled = text
                .contains("Early unexpected exit, operation never finished bootstrapping")
                && text.contains("The test runner crashed while preparing to run tests:")
                && text.contains("-[XCTWaiter(StallHandling) handleStalledWait:]");
            ensure!(
                stalled || text == "The test runner timed out while preparing to run tests.",
                "Unrecognized runner failure"
            );
            let matching: Vec<_> = found
                .iter()
                .filter(|(node, _)| node["nodeIdentifier"] == id)
                .collect();
            ensure!(matching.len() == 1, "Inconsistent runner test identity");
            let node = matching[0].0;
            ensure!(
                node["name"] == id && node["result"] == "Failed",
                "Inconsistent failure case"
            );
            let messages: Vec<_> = node["children"]
                .as_array()
                .context("Missing failure messages")?
                .iter()
                .filter(|n| n["nodeType"] == "Failure Message")
                .map(|n| n["name"].as_str())
                .collect();
            ensure!(messages == [Some(text)], "Failure messages disagree");
        }
        Ok(())
    }
    assess(summary, tests).is_ok()
}
