use std::{fs, process::Command};

use serde_json::{Map, Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

#[tokio::test]
async fn cli_emits_bounded_grafana_logs_as_ndjson() {
    let server = MockServer::start().await;
    let token_env = "MOWZ_TEST_GRAFANA_TOKEN_HTTP_SLICE";

    let times = (0..101)
        .map(|second| format!("2026-08-18T12:00:{:02}Z", second % 60))
        .collect::<Vec<_>>();
    let lines = (0..101)
        .map(|number| format!("line {number}"))
        .collect::<Vec<_>>();

    Mock::given(method("POST"))
        .and(path("/api/ds/query"))
        .and(header("authorization", "Bearer secret-token"))
        .and(body_json(json!({
            "queries": [{
                "refId": "A",
                "datasource": { "uid": "victoria-logs" },
                "expr": "_stream:{app=\"api\"}",
                "extraFilters": "_stream:{environment=\"production\"}",
                "queryType": "range",
                "maxLines": 101,
            }],
            "from": "now-6h",
            "to": "now-30m",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": { "A": { "frames": [{
                "schema": { "fields": [{"name": "Time"}, {"name": "Line"}] },
                "data": { "values": [times, lines] }
            }] } }
        })))
        .mount(&server)
        .await;

    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join(".mowz.toml");
    fs::write(
        &config_path,
        format!(
            r#"[projects.api]
type = "victoria_logs"
url = "{}"
datasource_uid = "victoria-logs"
token_env = "{token_env}"
scope_filter = "_stream:{{environment=\"production\"}}"
"#,
            server.uri()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .args([
            "--from",
            "now-6h",
            "--to",
            "now-30m",
            "api",
            "_stream:{app=\"api\"}",
        ])
        .current_dir(directory.path())
        .env(token_env, "secret-token")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert!(output.stdout.ends_with(b"\n"));

    let lines = output.stdout[..output.stdout.len() - 1].split(|byte| *byte == b'\n');
    let entries = lines
        .map(|line| serde_json::from_slice::<Map<String, Value>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 100);
    assert_eq!(
        Value::Object(entries[0].clone()),
        json!({"Time": "2026-08-18T12:00:00Z", "Line": "line 0"})
    );
    assert!(entries.iter().all(|entry| {
        !entry.contains_key("backend")
            && !entry.contains_key("errors")
            && !entry.contains_key("truncated")
    }));
}
