mod grafana;
mod railway;

use std::{collections::BTreeMap, env, fs, path::Path};

use eyre::{Context, Result, bail};
use railway::{RailwayAuth, RailwayScope};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value};

const RESULT_LIMIT: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeRange {
    from: String,
    to: String,
}

impl TimeRange {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }

    pub(crate) fn from(&self) -> &str {
        &self.from
    }

    pub(crate) fn to(&self) -> &str {
        &self.to
    }
}

impl Default for TimeRange {
    fn default() -> Self {
        Self::new("now-1h", "now")
    }
}

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
        url: String,
        datasource_uid: String,
        token_env: String,
        scope_filter: Option<String>,
    },
    Railway {
        environment_id: String,
        #[serde(default)]
        scope: RailwayScope,
        service_id: Option<String>,
        token_env: String,
        auth: RailwayAuth,
    },
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
    time_range: &TimeRange,
    client: &Client,
) -> Result<Vec<Map<String, Value>>> {
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

    let entries = match &project.backends[0] {
        Backend::VictoriaLogs {
            url,
            datasource_uid,
            token_env,
            scope_filter,
        } => {
            let token = read_token(token_env)?;
            let response = grafana::query(
                client,
                url,
                datasource_uid,
                &token,
                query,
                scope_filter.as_deref(),
                time_range,
            )
            .await?;
            bound_entries(grafana::extract_entries(&response), false)
        }
        Backend::Railway {
            environment_id,
            scope,
            service_id,
            token_env,
            auth,
        } => {
            let filter = scope.filter(service_id.as_deref(), query)?;
            let token = read_token(token_env)?;
            let response =
                railway::query_logs(client, &token, *auth, environment_id, &filter, time_range)
                    .await?;
            bound_entries(railway::extract_entries(&response), true)
        }
    };

    Ok(entries)
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
) -> Vec<Map<String, Value>> {
    if entries.len() > RESULT_LIMIT && retain_newest {
        entries.drain(..entries.len() - RESULT_LIMIT);
    } else {
        entries.truncate(RESULT_LIMIT);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

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
                environment_id,
                scope: RailwayScope::Service,
                service_id,
                token_env,
                auth: RailwayAuth::ProjectToken,
            } if environment_id == "environment-id"
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
}
