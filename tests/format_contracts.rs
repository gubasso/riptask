use insta::assert_snapshot;
use riptask::config::load_layer;
use riptask::domain::backend_state::BackendState;
use riptask::domain::id_map::IdMap;
use riptask::storage::frontmatter;

fn round_trip_issue(name: &str) -> String {
    let path = format!("tests/fixtures/issues/{name}");
    let content = std::fs::read_to_string(&path).expect("read fixture");
    let document = frontmatter::parse_issue_str(&path, &content).expect("parse issue");
    frontmatter::serialize_issue(&document).expect("serialize issue")
}

fn canonical_json<T: serde::Serialize>(value: &T) -> String {
    let value = serde_json::to_value(value).expect("serialize to value");
    let value = sort_json_value(value);
    serde_json::to_string_pretty(&value).expect("serialize sorted json")
}

fn sort_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (key, value) in map {
                sorted.insert(key, sort_json_value(value));
            }
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sort_json_value).collect())
        }
        other => other,
    }
}

#[test]
fn issue_fixtures_round_trip() {
    assert_snapshot!("gl_chr_wor_42", round_trip_issue("GL-CHR-WOR--42.md"));
    assert_snapshot!("gh_pen_fis_17", round_trip_issue("GH-PEN-FIS--17.md"));
    assert_snapshot!("lo_ice_1", round_trip_issue("LO-ICE--1.md"));
    assert_snapshot!(
        "gl_chr_wor_42_remote",
        round_trip_issue("GL-CHR-WOR--42.REMOTE.md")
    );
}

#[test]
fn config_fixtures_round_trip() {
    let config = load_layer(std::path::Path::new("tests/fixtures/config.yaml"))
        .expect("load config")
        .expect("config layer")
        .finalize()
        .expect("finalize config");
    assert_snapshot!(
        "riptask_yaml",
        serde_yaml_ng::to_string(&config).expect("serialize config")
    );
    let multi = load_layer(std::path::Path::new("tests/fixtures/config_multi.yaml"))
        .expect("load multi config")
        .expect("multi config layer")
        .finalize()
        .expect("finalize multi config");
    assert_snapshot!(
        "riptask_multi_yaml",
        serde_yaml_ng::to_string(&multi).expect("serialize multi config")
    );
}

#[test]
fn cache_fixtures_round_trip() {
    let backend_state: BackendState = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/backend_state.json").expect("backend state"),
    )
    .expect("parse backend state");
    assert_snapshot!("backend_state", canonical_json(&backend_state));

    let id_map: IdMap = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/id_map.json").expect("id map"),
    )
    .expect("parse id map");
    assert_snapshot!("id_map", canonical_json(&id_map));
}
