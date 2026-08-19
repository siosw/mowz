use eyre::{Context, Result, bail};
use reqwest::{Client, header::CONTENT_TYPE};
use serde_json::{Map, Value, json};

use crate::RESULT_LIMIT;

pub(crate) async fn query(
    client: &Client,
    grafana_url: &str,
    datasource_uid: &str,
    token: &str,
    query: &str,
    scope_filter: Option<&str>,
) -> Result<Value> {
    let endpoint = format!("{}/api/ds/query", grafana_url.trim_end_matches('/'));
    let mut query_model = json!({
        "refId": "A",
        "datasource": { "uid": datasource_uid },
        "expr": query,
        "queryType": "range",
        "maxLines": RESULT_LIMIT + 1,
    });
    if let Some(scope_filter) = scope_filter {
        query_model["extraFilters"] = json!(scope_filter);
    }
    let payload = json!({
        "queries": [query_model],
        "from": "now-1h",
        "to": "now",
    });

    let response = client
        .post(endpoint)
        .bearer_auth(token)
        .header(CONTENT_TYPE, "application/json")
        .json(&payload)
        .send()
        .await
        .context("failed to query Grafana")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Grafana response body")?;

    if !status.is_success() {
        bail!("Grafana query failed with status {status}: {body}");
    }

    serde_json::from_str(&body).context("failed to parse Grafana response as JSON")
}

pub(crate) fn extract_entries(response: &Value) -> Vec<Map<String, Value>> {
    response
        .get("results")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|results| results.values())
        .flat_map(extract_result_entries)
        .collect()
}

fn extract_result_entries(result: &Value) -> Vec<Map<String, Value>> {
    result
        .get("frames")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(extract_frame_entries)
        .collect()
}

fn extract_frame_entries(frame: &Value) -> Vec<Map<String, Value>> {
    let Some(fields) = frame
        .get("schema")
        .and_then(|schema| schema.get("fields"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let Some(values) = frame
        .get("data")
        .and_then(|data| data.get("values"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let row_count = values
        .iter()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .max()
        .unwrap_or(0);

    (0..row_count)
        .filter_map(|row| {
            let entry = fields
                .iter()
                .enumerate()
                .filter_map(|(column, field)| {
                    let name = field
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("field_{column}"));
                    let value = values
                        .get(column)
                        .and_then(Value::as_array)
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

    #[test]
    fn parses_grafana_frames_into_entries() {
        let response: Value =
            serde_json::from_str(include_str!("../tests/fixtures/grafana-response.json")).unwrap();

        assert_eq!(
            extract_entries(&response),
            vec![Map::from_iter([
                ("Line".to_owned(), json!("request completed")),
                ("Time".to_owned(), json!("2026-08-18T12:00:00Z")),
            ])]
        );
    }
}
