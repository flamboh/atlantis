use serde_json::Value;
use std::{fs, path::Path, process::Command};

const ABSOLUTE_TOLERANCE: f64 = 1e-10;
const RELATIVE_TOLERANCE: f64 = 1e-10;
const FIXTURE_NAMES: &[&str] = &[
    "clustered",
    "random",
    "mixed",
    "threshold-ancestor",
    "balanced-branches",
];

#[test]
fn maad_fixtures_match_haskell_goldens() {
    for name in FIXTURE_NAMES {
        assert_fixture_matches_golden(name);
    }
}

fn assert_fixture_matches_golden(name: &str) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/maad-conformance")
        .join(name)
        .join("input.txt");
    let golden = fixture.with_file_name("golden.json");

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args(["maad"])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let actual: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{name}: netflow-db emitted invalid JSON: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(&golden).unwrap_or_else(|error| {
            panic!("{name}: unable to read {}: {error}", golden.display())
        }))
        .unwrap_or_else(|error| panic!("{name}: invalid Haskell golden JSON: {error}"));

    assert_metadata_matches(name, &actual, &expected);
    assert_numeric_rows(
        name,
        "structure",
        &actual,
        &expected,
        &["q", "tauTilde", "sd"],
    );
    assert_numeric_rows(name, "spectrum", &actual, &expected, &["alpha", "f"]);
    assert_numeric_rows(name, "dimensions", &actual, &expected, &["q", "dim"]);
}

fn assert_metadata_matches(name: &str, actual: &Value, expected: &Value) {
    let actual_metadata = &actual["metadata"];
    let expected_metadata = &expected["metadata"];
    assert_eq!(
        prefix_lengths(actual_metadata),
        prefix_lengths(expected_metadata),
        "{name}: retained prefix levels differ"
    );
    assert_eq!(
        numeric_metadata(actual_metadata, "totalAddrs"),
        numeric_metadata(expected_metadata, "totalAddrs"),
        "{name}: total address counts differ"
    );
}

fn prefix_lengths(metadata: &Value) -> Vec<u64> {
    if let Some(lengths) = metadata.get("prefixLengths") {
        return lengths
            .as_array()
            .expect("prefixLengths must be an array")
            .iter()
            .map(|length| length.as_u64().expect("prefix length must be an integer"))
            .collect();
    }
    metadata["prefix_counts"]
        .as_array()
        .expect("prefix_counts must be an array")
        .iter()
        .map(|prefix| {
            prefix["pl"]
                .as_u64()
                .expect("prefix level must be an integer")
        })
        .collect()
}

fn numeric_metadata(metadata: &Value, key: &str) -> u64 {
    metadata[key]
        .as_u64()
        .unwrap_or_else(|| panic!("metadata.{key} must be an integer"))
}

fn assert_numeric_rows(
    name: &str,
    section: &str,
    actual: &Value,
    expected: &Value,
    fields: &[&str],
) {
    let actual_rows = actual[section]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: {section} must be an array"));
    let expected_rows = expected[section]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: golden {section} must be an array"));
    assert_eq!(
        actual_rows.len(),
        expected_rows.len(),
        "{name}: {section} row count differs"
    );

    for (row_index, (actual_row, expected_row)) in actual_rows.iter().zip(expected_rows).enumerate()
    {
        for field in fields {
            let actual_number = actual_row[field]
                .as_f64()
                .unwrap_or_else(|| panic!("{name}: {section}[{row_index}].{field} is not numeric"));
            let expected_number = expected_row[field].as_f64().unwrap_or_else(|| {
                panic!("{name}: golden {section}[{row_index}].{field} is not numeric")
            });
            let difference = (actual_number - expected_number).abs();
            let tolerance = ABSOLUTE_TOLERANCE
                + RELATIVE_TOLERANCE * actual_number.abs().max(expected_number.abs());
            assert!(
                difference <= tolerance,
                "{name}: {section}[{row_index}].{field} differs: actual={actual_number:?}, expected={expected_number:?}, difference={difference:?}, tolerance={tolerance:?}"
            );
        }
    }
}
