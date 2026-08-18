use std::{collections::BTreeMap, env, fs, path::Path};

use chrono::{SecondsFormat, TimeDelta, Utc};
use eyre::{Context, Result, bail};
use reqwest::{Client, header::CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const RESULT_LIMIT: usize = 100;
const RAILWAY_API_URL: &str = "https://backboard.railway.com/graphql/v2";

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
    Railway {
        name: String,
        project_id: String,
        environment_id: String,
        service_id: String,
        token_env: String,
        auth: RailwayAuth,
    },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RailwayAuth {
    ProjectToken,
    Bearer,
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
        toml::from_str(&contents).wrap_err_with(|| format!("failed to parse {}", path.display()))
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

    let (name, entries, truncated) = match &project.backends[0] {
        Backend::VictoriaLogs {
            name,
            url,
            datasource_uid,
            token_env,
        } => {
            let token = read_token(token_env)?;
            let response = query_grafana(client, url, datasource_uid, &token, query).await?;
            let (entries, truncated) = bound_entries(extract_entries(&response), false);
            (name, entries, truncated)
        }
        Backend::Railway {
            name,
            project_id,
            environment_id,
            service_id,
            token_env,
            auth,
        } => {
            let token = read_token(token_env)?;
            let deployment_id = resolve_railway_deployment(
                client,
                RAILWAY_API_URL,
                &token,
                *auth,
                project_id,
                environment_id,
                service_id,
            )
            .await?;
            let response = query_railway_logs(
                client,
                RAILWAY_API_URL,
                &token,
                *auth,
                &deployment_id,
                query,
            )
            .await?;
            let (entries, truncated) = bound_entries(extract_railway_entries(&response), true);
            (name, entries, truncated)
        }
    };

    Ok(Output {
        results: vec![BackendResult {
            backend: name.clone(),
            entries,
        }],
        errors: Vec::new(),
        truncated,
    })
}

fn read_token(token_env: &str) -> Result<String> {
    let token = env::var(token_env)
        .wrap_err_with(|| format!("environment variable {token_env} is not set"))?;
    if token.is_empty() {
        bail!("environment variable {token_env} is empty");
    }
    Ok(token)
}

fn bound_entries(
    mut entries: Vec<Map<String, Value>>,
    retain_newest: bool,
) -> (Vec<Map<String, Value>>, bool) {
    let truncated = entries.len() > RESULT_LIMIT;
    if truncated && retain_newest {
        entries.drain(..entries.len() - RESULT_LIMIT);
    } else {
        entries.truncate(RESULT_LIMIT);
    }
    (entries, truncated)
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

async fn resolve_railway_deployment(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
) -> Result<String> {
    let query = r#"query Deployments($input: DeploymentListInput!) {
  deployments(input: $input) {
    edges { node { id createdAt status } }
  }
}"#;
    let response = query_railway_api(
        client,
        api_url,
        token,
        auth,
        query,
        json!({
            "input": {
                "projectId": project_id,
                "environmentId": environment_id,
                "serviceId": service_id,
            }
        }),
    )
    .await?;

    let mut deployments = response
        .pointer("/data/deployments/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .collect::<Vec<_>>();
    deployments.sort_by(|left, right| {
        right
            .get("createdAt")
            .and_then(Value::as_str)
            .cmp(&left.get("createdAt").and_then(Value::as_str))
    });

    let deployment = deployments
        .iter()
        .copied()
        .find(|deployment| {
            deployment
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("success"))
        })
        .or_else(|| deployments.first().copied())
        .ok_or_else(|| eyre::eyre!("Railway returned no deployments for the configured service"))?;

    deployment
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| eyre::eyre!("Railway deployment is missing an id"))
}

async fn query_railway_logs(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    deployment_id: &str,
    filter: &str,
) -> Result<Value> {
    let query = r#"query DeploymentLogs($deploymentId: String!, $limit: Int, $filter: String, $startDate: DateTime) {
  deploymentLogs(deploymentId: $deploymentId, limit: $limit, filter: $filter, startDate: $startDate) {
    timestamp
    message
    severity
  }
}"#;
    let start_date = (Utc::now() - TimeDelta::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);

    query_railway_api(
        client,
        api_url,
        token,
        auth,
        query,
        json!({
            "deploymentId": deployment_id,
            "limit": RESULT_LIMIT + 1,
            "filter": filter,
            "startDate": start_date,
        }),
    )
    .await
}

async fn query_railway_api(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    query: &str,
    variables: Value,
) -> Result<Value> {
    let request = client
        .post(api_url)
        .header(CONTENT_TYPE, "application/json");
    let request = match auth {
        RailwayAuth::ProjectToken => request.header("Project-Access-Token", token),
        RailwayAuth::Bearer => request.bearer_auth(token),
    };
    let response = request
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .context("failed to query Railway")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Railway response body")?;
    if !status.is_success() {
        bail!("Railway query failed with status {status}: {body}");
    }

    let response: Value =
        serde_json::from_str(&body).context("failed to parse Railway response as JSON")?;
    if response
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        bail!(
            "Railway query returned GraphQL errors: {}",
            response["errors"]
        );
    }
    Ok(response)
}

fn extract_railway_entries(response: &Value) -> Vec<Map<String, Value>> {
    response
        .pointer("/data/deploymentLogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|entry| {
            ["timestamp", "message", "severity"]
                .into_iter()
                .filter_map(|field| {
                    entry
                        .get(field)
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(|value| (field.to_owned(), value))
                })
                .collect()
        })
        .filter(|entry: &Map<String, Value>| !entry.is_empty())
        .collect()
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
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header, method, path},
    };

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

    #[test]
    fn parses_railway_logs_into_entries() {
        let response: Value =
            serde_json::from_str(include_str!("../tests/fixtures/railway-response.json")).unwrap();

        assert_eq!(
            extract_railway_entries(&response),
            vec![Map::from_iter([
                ("message".to_owned(), json!("request completed")),
                ("severity".to_owned(), json!("info")),
                ("timestamp".to_owned(), json!("2026-08-18T12:00:00Z")),
            ])]
        );
    }

    #[test]
    fn parses_railway_backend_config() {
        let config: Config = toml::from_str(
            r#"[projects.api]

[[projects.api.backends]]
name = "railway-production"
type = "railway"
project_id = "project-id"
environment_id = "environment-id"
service_id = "service-id"
token_env = "RAILWAY_TOKEN"
auth = "project_token"
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["api"].backends[0],
            Backend::Railway {
                name,
                project_id,
                environment_id,
                service_id,
                token_env,
                auth: RailwayAuth::ProjectToken,
            } if name == "railway-production"
                && project_id == "project-id"
                && environment_id == "environment-id"
                && service_id == "service-id"
                && token_env == "RAILWAY_TOKEN"
        ));
    }

    #[tokio::test]
    async fn queries_latest_successful_railway_deployment_and_bounds_logs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("project-access-token", "secret-token"))
            .and(body_string_contains("query Deployments("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "deployments": { "edges": [
                    { "node": {
                        "id": "failed-deployment",
                        "createdAt": "2026-08-18T12:00:00Z",
                        "status": "FAILED"
                    } },
                    { "node": {
                        "id": "successful-deployment",
                        "createdAt": "2026-08-18T11:00:00Z",
                        "status": "SUCCESS"
                    } }
                ] } }
            })))
            .mount(&server)
            .await;

        let logs = (0..101)
            .map(|number| {
                json!({
                    "timestamp": format!("2026-08-18T12:00:{:02}Z", number % 60),
                    "message": format!("line {number}"),
                    "severity": "info",
                })
            })
            .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("project-access-token", "secret-token"))
            .and(body_string_contains("query DeploymentLogs("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "deploymentLogs": logs }
            })))
            .mount(&server)
            .await;

        let api_url = format!("{}/graphql/v2", server.uri());
        let client = Client::new();
        let deployment_id = resolve_railway_deployment(
            &client,
            &api_url,
            "secret-token",
            RailwayAuth::ProjectToken,
            "project-id",
            "environment-id",
            "service-id",
        )
        .await
        .unwrap();
        assert_eq!(deployment_id, "successful-deployment");

        let response = query_railway_logs(
            &client,
            &api_url,
            "secret-token",
            RailwayAuth::ProjectToken,
            &deployment_id,
            "@level:error AND timeout",
        )
        .await
        .unwrap();
        let (entries, truncated) = bound_entries(extract_railway_entries(&response), true);
        assert!(truncated);
        assert_eq!(entries.len(), 100);
        assert_eq!(entries[0]["message"], "line 1");
        assert_eq!(entries[99]["message"], "line 100");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        let deployment_request: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            deployment_request["variables"],
            json!({
                "input": {
                    "projectId": "project-id",
                    "environmentId": "environment-id",
                    "serviceId": "service-id",
                }
            })
        );
        let logs_request: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(
            logs_request["variables"]["deploymentId"],
            "successful-deployment"
        );
        assert_eq!(logs_request["variables"]["limit"], 101);
        assert_eq!(
            logs_request["variables"]["filter"],
            "@level:error AND timeout"
        );
        let start_date = logs_request["variables"]["startDate"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let age = Utc::now() - start_date;
        assert!(age >= TimeDelta::minutes(59) && age <= TimeDelta::minutes(61));
    }

    #[tokio::test]
    async fn reports_railway_graphql_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "errors": [{ "message": "not authorized" }]
            })))
            .mount(&server)
            .await;

        let error = query_railway_api(
            &Client::new(),
            &format!("{}/graphql/v2", server.uri()),
            "secret-token",
            RailwayAuth::Bearer,
            "query Test { me { id } }",
            json!({}),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Railway query returned GraphQL errors")
        );
    }
}
