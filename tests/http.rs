use std::{env, fs};

use ctx::{Config, query_project};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

#[tokio::test]
async fn queries_grafana_with_configured_credentials_and_bounds_output() {
    let server = MockServer::start().await;
    let token_env = "CTX_TEST_GRAFANA_TOKEN_HTTP_SLICE";
    // This test is the only environment-mutating test in the suite.
    unsafe { env::set_var(token_env, "secret-token") };

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
            "from": "now-1h",
            "to": "now",
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
    let config_path = directory.path().join(".ctx.toml");
    fs::write(
        &config_path,
        format!(
            r#"[projects.api]

[[projects.api.backends]]
name = "production"
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

    let config = Config::load(&config_path).unwrap();
    let output = query_project(
        &config,
        "api",
        "_stream:{app=\"api\"}",
        &reqwest::Client::new(),
    )
    .await
    .unwrap();

    let output = serde_json::to_value(output).unwrap();
    assert_eq!(output["results"][0]["backend"], "production");
    assert_eq!(
        output["results"][0]["entries"].as_array().unwrap().len(),
        100
    );
    assert_eq!(
        output["results"][0]["entries"][0],
        json!({"Time": "2026-08-18T12:00:00Z", "Line": "line 0"})
    );
    assert_eq!(output["errors"], json!([]));
    assert_eq!(output["truncated"], true);

    unsafe { env::remove_var(token_env) };
}
