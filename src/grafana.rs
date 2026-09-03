use std::collections::BTreeMap;

use eyre::Result;
use reqwest::{Client, header::CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{QueryOptions, http::parse_json_response};

#[derive(Deserialize)]
struct GrafanaResponse {
    results: BTreeMap<String, GrafanaResult>,
}

#[derive(Deserialize)]
struct GrafanaResult {
    frames: Vec<GrafanaFrame>,
}

#[derive(Deserialize)]
struct GrafanaFrame {
    schema: GrafanaSchema,
    data: GrafanaData,
}

#[derive(Deserialize)]
struct GrafanaSchema {
    fields: Vec<GrafanaField>,
}

#[derive(Deserialize)]
struct GrafanaField {
    name: Option<String>,
}

#[derive(Deserialize)]
struct GrafanaData {
    values: Vec<Vec<Value>>,
}

pub(crate) async fn query(
    client: &Client,
    grafana_url: &str,
    datasource_uid: &str,
    token: &str,
    query: &str,
    scope_filter: Option<&str>,
    options: &QueryOptions<'_>,
) -> Result<Vec<Map<String, Value>>> {
    let endpoint = format!("{}/api/ds/query", grafana_url.trim_end_matches('/'));
    let mut query_model = json!({
        "refId": "A",
        "datasource": { "uid": datasource_uid },
        "expr": query,
        "queryType": "range",
        "maxLines": options.limit,
    });
    if let Some(scope_filter) = scope_filter {
        query_model["extraFilters"] = json!(scope_filter);
    }
    let payload = json!({
        "queries": [query_model],
        "from": options.time_range.from(),
        "to": options.time_range.to(),
    });

    let response = parse_json_response(
        client
            .post(endpoint)
            .bearer_auth(token)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await,
        "Grafana",
    )
    .await?;
    Ok(extract_entries(response))
}

fn extract_entries(response: GrafanaResponse) -> Vec<Map<String, Value>> {
    response
        .results
        .into_values()
        .flat_map(extract_result_entries)
        .collect()
}

fn extract_result_entries(result: GrafanaResult) -> Vec<Map<String, Value>> {
    result
        .frames
        .into_iter()
        .flat_map(extract_frame_entries)
        .collect()
}

fn extract_frame_entries(frame: GrafanaFrame) -> Vec<Map<String, Value>> {
    let row_count = frame.data.values.iter().map(Vec::len).max().unwrap_or(0);

    (0..row_count)
        .filter_map(|row| {
            let entry = frame
                .schema
                .fields
                .iter()
                .enumerate()
                .filter_map(|(column, field)| {
                    let name = field
                        .name
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("field_{column}"));
                    let value = frame
                        .data
                        .values
                        .get(column)
                        .and_then(|column| column.get(row))
                        .filter(|value| !value.is_null())?
                        .clone();
                    Some((name, value))
                })
                .collect::<Map<_, _>>();
            (!entry.is_empty()).then_some(entry)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TimeRange;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[test]
    fn parses_grafana_frames_into_entries() {
        let response: GrafanaResponse =
            serde_json::from_str(include_str!("../tests/fixtures/grafana-response.json")).unwrap();

        assert_eq!(
            extract_entries(response),
            vec![Map::from_iter([
                ("Line".to_owned(), json!("request completed")),
                ("Time".to_owned(), json!("2026-08-18T12:00:00Z")),
            ])]
        );
    }

    #[test]
    fn accepts_empty_grafana_results() {
        let response: GrafanaResponse = serde_json::from_value(json!({ "results": {} })).unwrap();

        assert!(extract_entries(response).is_empty());
    }

    #[tokio::test]
    async fn reports_grafana_response_schema_drift() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/ds/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": { "A": { "series": [] } }
            })))
            .mount(&server)
            .await;

        let time_range = TimeRange::default();
        let error = query(
            &Client::new(),
            &server.uri(),
            "victoria-logs",
            "token",
            "query",
            None,
            &QueryOptions {
                time_range: &time_range,
                limit: 3,
            },
        )
        .await
        .unwrap_err();

        let error = format!("{error:?}");
        assert!(error.contains("failed to parse Grafana response as the expected schema"));
        assert!(error.contains("missing field `frames`"));
    }

    #[tokio::test]
    async fn parses_successful_grafana_responses() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/ds/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": {} })))
            .mount(&server)
            .await;

        let time_range = TimeRange::default();
        let response = query(
            &Client::new(),
            &server.uri(),
            "victoria-logs",
            "token",
            "query",
            None,
            &QueryOptions {
                time_range: &time_range,
                limit: 3,
            },
        )
        .await
        .unwrap();

        assert!(response.is_empty());
    }

    #[tokio::test]
    async fn reports_invalid_grafana_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/ds/query"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not JSON"))
            .mount(&server)
            .await;

        let time_range = TimeRange::default();
        let error = query(
            &Client::new(),
            &server.uri(),
            "victoria-logs",
            "token",
            "query",
            None,
            &QueryOptions {
                time_range: &time_range,
                limit: 3,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "failed to parse Grafana response as the expected schema"
        );
    }

    #[tokio::test]
    async fn transport_errors_do_not_expose_the_grafana_url() {
        let time_range = TimeRange::default();
        let error = query(
            &Client::new(),
            "http://127.0.0.1:0/sentinel-secret",
            "victoria-logs",
            "token",
            "query",
            None,
            &QueryOptions {
                time_range: &time_range,
                limit: 3,
            },
        )
        .await
        .unwrap_err();

        assert!(!format!("{error:?}").contains("sentinel-secret"));
    }

    #[tokio::test]
    async fn bounds_grafana_error_response_bodies() {
        let server = MockServer::start().await;
        let body = format!("useful beginning: {}sentinel tail", "x".repeat(2048));
        Mock::given(method("POST"))
            .and(path("/api/ds/query"))
            .respond_with(ResponseTemplate::new(502).set_body_string(body))
            .mount(&server)
            .await;

        let time_range = TimeRange::default();
        let error = query(
            &Client::new(),
            &server.uri(),
            "victoria-logs",
            "token",
            "query",
            None,
            &QueryOptions {
                time_range: &time_range,
                limit: 3,
            },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("status 502 Bad Gateway: useful beginning"));
        assert!(error.ends_with("... [truncated]"));
        assert!(!error.contains("sentinel tail"));
    }
}
