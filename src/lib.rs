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
        scope_filter: Option<String>,
    },
    Railway {
        name: String,
        environment_id: String,
        #[serde(default)]
        scope: RailwayScope,
        service_id: Option<String>,
        token_env: String,
        auth: RailwayAuth,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RailwayScope {
    #[default]
    Service,
    Environment,
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
            scope_filter,
        } => {
            let token = read_token(token_env)?;
            let response = query_grafana(
                client,
                url,
                datasource_uid,
                &token,
                query,
                scope_filter.as_deref(),
            )
            .await?;
            let (entries, truncated) = bound_entries(extract_entries(&response), false);
            (name, entries, truncated)
        }
        Backend::Railway {
            name,
            environment_id,
            scope,
            service_id,
            token_env,
            auth,
        } => {
            let filter = scope.filter(service_id.as_deref(), query)?;
            let token = read_token(token_env)?;
            let response = query_railway_logs(
                client,
                RAILWAY_API_URL,
                &token,
                *auth,
                environment_id,
                &filter,
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

impl RailwayScope {
    fn filter(self, service_id: Option<&str>, filter: &str) -> Result<String> {
        match (self, service_id) {
            (Self::Service, Some(service_id)) if filter.is_empty() => {
                Ok(format!("@service:{service_id}"))
            }
            (Self::Service, Some(service_id)) => {
                Ok(format!("@service:{service_id} AND ({filter})"))
            }
            (Self::Service, None) => {
                bail!("Railway service scope requires service_id")
            }
            (Self::Environment, None) => Ok(filter.to_owned()),
            (Self::Environment, Some(_)) => {
                bail!("Railway environment scope must not configure service_id")
            }
        }
    }
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

async fn query_railway_logs(
    client: &Client,
    api_url: &str,
    token: &str,
    auth: RailwayAuth,
    environment_id: &str,
    filter: &str,
) -> Result<Value> {
    let query = r#"query EnvironmentLogs($environmentId: String!, $filter: String, $beforeDate: String!, $anchorDate: String!, $afterDate: String!, $beforeLimit: Int!, $afterLimit: Int!) {
  environmentLogs(environmentId: $environmentId, filter: $filter, beforeDate: $beforeDate, anchorDate: $anchorDate, afterDate: $afterDate, beforeLimit: $beforeLimit, afterLimit: $afterLimit) {
    timestamp
    message
    severity
    tags {
      serviceId
      deploymentId
    }
  }
}"#;
    let now = Utc::now();
    let start_date = (now - TimeDelta::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let end_date = now.to_rfc3339_opts(SecondsFormat::Secs, true);

    query_railway_api(
        client,
        api_url,
        token,
        auth,
        query,
        json!({
            "environmentId": environment_id,
            "filter": filter,
            "beforeDate": start_date,
            "anchorDate": end_date,
            "afterDate": end_date,
            "beforeLimit": RESULT_LIMIT + 1,
            "afterLimit": 0,
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
        .pointer("/data/environmentLogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|entry| {
            let mut selected = ["timestamp", "message", "severity"]
                .into_iter()
                .filter_map(|field| {
                    entry
                        .get(field)
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(|value| (field.to_owned(), value))
                })
                .collect::<Map<_, _>>();
            if let Some(tags) = entry.get("tags").and_then(Value::as_object) {
                selected.extend(
                    ["serviceId", "deploymentId"]
                        .into_iter()
                        .filter_map(|field| {
                            tags.get(field)
                                .filter(|value| !value.is_null())
                                .cloned()
                                .map(|value| (field.to_owned(), value))
                        }),
                );
            }
            selected
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
                ("deploymentId".to_owned(), json!("deployment-id")),
                ("message".to_owned(), json!("request completed")),
                ("severity".to_owned(), json!("info")),
                ("serviceId".to_owned(), json!("service-id")),
                ("timestamp".to_owned(), json!("2026-08-18T12:00:00Z")),
            ])]
        );
    }

    #[test]
    fn parses_legacy_railway_service_config() {
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
                environment_id,
                scope: RailwayScope::Service,
                service_id,
                token_env,
                auth: RailwayAuth::ProjectToken,
            } if name == "railway-production"
                && environment_id == "environment-id"
                && service_id.as_deref() == Some("service-id")
                && token_env == "RAILWAY_TOKEN"
        ));
    }

    #[test]
    fn parses_optional_victoria_logs_scope_filter() {
        let config: Config = toml::from_str(
            r#"[projects.scoped]

[[projects.scoped.backends]]
name = "scoped-production"
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token_env = "GRAFANA_TOKEN"
scope_filter = "_stream:{environment=\"production\"}"

[projects.unscoped]

[[projects.unscoped.backends]]
name = "unscoped-production"
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token_env = "GRAFANA_TOKEN"
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["scoped"].backends[0],
            Backend::VictoriaLogs {
                scope_filter: Some(scope_filter),
                ..
            } if scope_filter == "_stream:{environment=\"production\"}"
        ));
        assert!(matches!(
            &config.projects["unscoped"].backends[0],
            Backend::VictoriaLogs {
                scope_filter: None,
                ..
            }
        ));
    }

    #[test]
    fn parses_explicit_railway_environment_config() {
        let config: Config = toml::from_str(
            r#"[projects.api]

[[projects.api.backends]]
name = "railway-production"
type = "railway"
environment_id = "environment-id"
scope = "environment"
token_env = "RAILWAY_TOKEN"
auth = "bearer"
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["api"].backends[0],
            Backend::Railway {
                environment_id,
                scope: RailwayScope::Environment,
                service_id: None,
                auth: RailwayAuth::Bearer,
                ..
            } if environment_id == "environment-id"
        ));
    }

    #[test]
    fn builds_explicit_railway_scope_filters() {
        assert_eq!(
            RailwayScope::Service
                .filter(Some("service-id"), "@level:error OR timeout")
                .unwrap(),
            "@service:service-id AND (@level:error OR timeout)"
        );
        assert_eq!(
            RailwayScope::Environment
                .filter(None, "@level:error OR timeout")
                .unwrap(),
            "@level:error OR timeout"
        );
        assert_eq!(
            RailwayScope::Service
                .filter(None, "error")
                .unwrap_err()
                .to_string(),
            "Railway service scope requires service_id"
        );
        assert_eq!(
            RailwayScope::Environment
                .filter(Some("service-id"), "error")
                .unwrap_err()
                .to_string(),
            "Railway environment scope must not configure service_id"
        );
    }

    #[tokio::test]
    async fn queries_bounded_railway_environment_logs() {
        let server = MockServer::start().await;
        let logs = (0..101)
            .map(|number| {
                json!({
                    "timestamp": format!("2026-08-18T12:00:{:02}Z", number % 60),
                    "message": format!("line {number}"),
                    "severity": "info",
                    "tags": {
                        "serviceId": "service-id",
                        "deploymentId": format!("deployment-{number}"),
                    },
                })
            })
            .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path("/graphql/v2"))
            .and(header("project-access-token", "secret-token"))
            .and(body_string_contains("query EnvironmentLogs("))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "environmentLogs": logs }
            })))
            .mount(&server)
            .await;

        let api_url = format!("{}/graphql/v2", server.uri());
        let client = Client::new();
        let response = query_railway_logs(
            &client,
            &api_url,
            "secret-token",
            RailwayAuth::ProjectToken,
            "environment-id",
            "@service:service-id AND (@level:error OR timeout)",
        )
        .await
        .unwrap();
        let (entries, truncated) = bound_entries(extract_railway_entries(&response), true);
        assert!(truncated);
        assert_eq!(entries.len(), 100);
        assert_eq!(entries[0]["message"], "line 1");
        assert_eq!(entries[0]["serviceId"], "service-id");
        assert_eq!(entries[0]["deploymentId"], "deployment-1");
        assert_eq!(entries[99]["message"], "line 100");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let logs_request: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(logs_request["variables"]["environmentId"], "environment-id");
        assert_eq!(logs_request["variables"]["beforeLimit"], 101);
        assert_eq!(logs_request["variables"]["afterLimit"], 0);
        assert_eq!(
            logs_request["variables"]["filter"],
            "@service:service-id AND (@level:error OR timeout)"
        );
        let start_date = logs_request["variables"]["beforeDate"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let age = Utc::now() - start_date;
        assert!(age >= TimeDelta::minutes(59) && age <= TimeDelta::minutes(61));
        assert_eq!(
            logs_request["variables"]["anchorDate"],
            logs_request["variables"]["afterDate"]
        );
        let end_date = logs_request["variables"]["anchorDate"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let age = Utc::now() - end_date;
        assert!(age >= TimeDelta::zero() && age <= TimeDelta::minutes(1));
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
