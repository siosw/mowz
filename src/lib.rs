use std::{collections::BTreeMap, env, fs, path::Path};

use eyre::{Context, Result, bail};
use reqwest::{Client, header::CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const RESULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct Config {
    projects: BTreeMap<String, Project>,
}

#[derive(Debug, Deserialize)]
struct Project {
    backends: Vec<Backend>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Backend {
    VictoriaLogs {
        name: String,
        url: String,
        datasource_uid: String,
        token_env: String,
    },
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Output {
    results: Vec<BackendResult>,
    errors: Vec<Value>,
    truncated: bool,
}

#[derive(Debug, Serialize, PartialEq)]
struct BackendResult {
    backend: String,
    entries: Vec<Map<String, Value>>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;
        serde_yaml::from_str(&contents)
            .wrap_err_with(|| format!("failed to parse {}", path.display()))
    }
}

pub async fn query_project(
    config: &Config,
    project_name: &str,
    query: &str,
    client: &Client,
) -> Result<Output> {
    let project = config
        .projects
        .get(project_name)
        .ok_or_else(|| eyre::eyre!("project {project_name:?} is not configured"))?;

    if project.backends.len() != 1 {
        bail!(
            "project {project_name:?} must configure exactly one backend (found {})",
            project.backends.len()
        );
    }

    let backend = &project.backends[0];
    let Backend::VictoriaLogs {
        name,
        url,
        datasource_uid,
        token_env,
    } = backend;

    let token = env::var(token_env)
        .wrap_err_with(|| format!("environment variable {token_env} is not set"))?;
    if token.is_empty() {
        bail!("environment variable {token_env} is empty");
    }

    let response = query_grafana(client, url, datasource_uid, &token, query).await?;
    let mut entries = extract_entries(&response);
    let truncated = entries.len() > RESULT_LIMIT;
    entries.truncate(RESULT_LIMIT);

    Ok(Output {
        results: vec![BackendResult {
            backend: name.clone(),
            entries,
        }],
        errors: Vec::new(),
        truncated,
    })
}

async fn query_grafana(
    client: &Client,
    grafana_url: &str,
    datasource_uid: &str,
    token: &str,
    query: &str,
) -> Result<Value> {
    let endpoint = format!("{}/api/ds/query", grafana_url.trim_end_matches('/'));
    let payload = json!({
        "queries": [{
            "refId": "A",
            "datasource": { "uid": datasource_uid },
            "expr": query,
            "queryType": "range",
            "maxLines": RESULT_LIMIT + 1,
        }],
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

fn extract_entries(response: &Value) -> Vec<Map<String, Value>> {
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
