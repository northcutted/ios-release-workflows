use ios_release_native::version::decide;
use serde_json::{Value, json};

fn policy() -> Value {
    let legacy: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/legacy-release-policy.json"
    ))
    .unwrap();
    json!({"schema_version":1,"release_rules":legacy["plugins"][0][1]["releaseRules"],"note_types":legacy["plugins"][1][1]["presetConfig"]["types"]})
}
#[test]
fn native_versioning_matches_the_deployed_reference_cases() {
    let reference: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/version-reference.json")).unwrap();
    for case in reference["cases"].as_array().unwrap() {
        let expected = &case["expected"];
        let replacement = (case["name"] == "replacement")
            .then(|| json!({"version":"1.6.5","source_tag":"v1.6.5"}));
        let result = decide(
            &policy(),
            &case["commits"],
            Some("v1.6.5"),
            expected["source_sha"].as_str().unwrap(),
            "example/fixture",
            "DATE",
            replacement.as_ref(),
        )
        .unwrap();
        assert_eq!(result, *expected, "case {}", case["name"]);
    }
}
#[test]
fn policy_replacements_and_disabled_rules_fail_closed() {
    let commits = json!([{"hash":"a".repeat(40),"message":"feat(core): new feature"}]);
    let mut p = policy();
    p["release_rules"] = json!([{"scope":"core*","release":false}]);
    assert_eq!(
        decide(
            &p,
            &commits,
            Some("v1.6.5"),
            "head",
            "owner/app",
            "DATE",
            None
        )
        .unwrap()["will_release"],
        false
    );
    p["release_rules"] = json!([{"custom":"unsupported","release":"patch"}]);
    assert!(
        decide(
            &p,
            &commits,
            Some("v1.6.5"),
            "head",
            "owner/app",
            "DATE",
            None
        )
        .is_err()
    );
    let replacement = json!({"version":"1.6.5","source_tag":"v1.6.5"});
    let result = decide(
        &policy(),
        &commits,
        Some("v1.6.5"),
        "head",
        "owner/app",
        "DATE",
        Some(&replacement),
    )
    .unwrap();
    assert_eq!(result["version"], "1.6.5");
    assert!(result["git_tag"].is_null());
    assert!(
        decide(
            &policy(),
            &commits,
            Some("v1.7.0"),
            "head",
            "owner/app",
            "DATE",
            Some(&replacement)
        )
        .is_err()
    );
}

#[test]
fn actions_build_numbers_are_monotonic_bounded_and_never_fallback_to_colliding_apple_maximum() {
    use ios_release_native::version::github_number;
    let config = serde_json::json!({"build_number_offset":200});
    assert_eq!(github_number(&config, "110", "1", "1").unwrap(), "201.1");
    assert_eq!(github_number(&config, "110", "2", "1").unwrap(), "202.1");
    assert_eq!(github_number(&config, "110", "2", "2").unwrap(), "202.2");
    for (minimum, run, attempt) in [
        ("203", "2", "1"),
        ("1", "10000", "1"),
        ("1", "1", "100"),
        ("1", "0", "1"),
    ] {
        assert!(github_number(&config, minimum, run, attempt).is_err());
    }
}
