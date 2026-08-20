mod grafana;
mod railway;
mod time_range;

use std::{collections::BTreeMap, env, fs, path::Path, process::Command};

use eyre::{Context, Result, bail};
use railway::{RailwayAuth, RailwayScope};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value};
pub use time_range::TimeRange;

const RESULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct Config {
    projects: BTreeMap<String, Backend>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Backend {
    VictoriaLogs {
        url: StringValue,
        datasource_uid: StringValue,
        token: StringValue,
        scope_filter: Option<StringValue>,
    },
    Railway {
        environment_id: StringValue,
        #[serde(default)]
        scope: RailwayScope,
        service_id: Option<StringValue>,
        token: StringValue,
        auth: RailwayAuth,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringValue {
    Literal(String),
    Source {
        env: Option<String>,
        op: Option<String>,
    },
}

impl StringValue {
    fn resolve(&self) -> Result<String> {
        self.resolve_with(|name| env::var(name), read_op)
    }

    fn resolve_with<E, O>(&self, mut read_env: E, mut read_op: O) -> Result<String>
    where
        E: FnMut(&str) -> std::result::Result<String, env::VarError>,
        O: FnMut(&str) -> Result<String>,
    {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Source { env, op } => {
                if let Some(name) = env {
                    match read_env(name) {
                        Ok(value) => return Ok(value),
                        Err(env::VarError::NotPresent) => {}
                        Err(env::VarError::NotUnicode(_)) => {
                            bail!("environment variable {name} is not valid UTF-8")
                        }
                    }
                }

                if let Some(reference) = op {
                    return read_op(reference);
                }

                if let Some(name) = env {
                    bail!(
                        "environment variable {name} is not set and no 1Password fallback is configured"
                    );
                }

                bail!("string source must configure `env`, `op`, or both")
            }
        }
    }
}

fn read_op(reference: &str) -> Result<String> {
    let output = Command::new("op")
        .args(["read", "--no-newline", reference])
        .output()
        .wrap_err(
            "failed to run `op read`; install the 1Password CLI and authenticate it, or provide the configured environment variable",
        )?;

    if !output.status.success() {
        bail!(
            "`op read` failed; check that the 1Password CLI is authenticated and the configured reference is accessible"
        );
    }

    String::from_utf8(output.stdout)
        .wrap_err("`op read` returned a configured value that is not valid UTF-8")
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
    let backend = config
        .projects
        .get(project_name)
        .ok_or_else(|| eyre::eyre!("project {project_name:?} is not configured"))?;

    let entries = match backend {
        Backend::VictoriaLogs {
            url,
            datasource_uid,
            token,
            scope_filter,
        } => {
            let url = url.resolve().wrap_err("failed to resolve `url`")?;
            let datasource_uid = datasource_uid
                .resolve()
                .wrap_err("failed to resolve `datasource_uid`")?;
            let token = read_token(token)?;
            let scope_filter = scope_filter
                .as_ref()
                .map(StringValue::resolve)
                .transpose()
                .wrap_err("failed to resolve `scope_filter`")?;
            let response = grafana::query(
                client,
                &url,
                &datasource_uid,
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
            token,
            auth,
        } => {
            let environment_id = environment_id
                .resolve()
                .wrap_err("failed to resolve `environment_id`")?;
            let service_id = service_id
                .as_ref()
                .map(StringValue::resolve)
                .transpose()
                .wrap_err("failed to resolve `service_id`")?;
            let filter = scope.filter(service_id.as_deref(), query)?;
            let token = read_token(token)?;
            let response =
                railway::query_logs(client, &token, *auth, &environment_id, &filter, time_range)
                    .await?;
            bound_entries(railway::extract_entries(&response), true)
        }
    };

    Ok(entries)
}

fn read_token(value: &StringValue) -> Result<String> {
    let token = value.resolve().wrap_err("failed to resolve `token`")?;
    if token.is_empty() {
        bail!("configured token resolved to an empty value");
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

    #[derive(Deserialize)]
    struct TestValue {
        value: StringValue,
    }

    #[test]
    fn parses_railway_service_config() {
        let config: Config = toml::from_str(
            r#"[projects.api]
name = "railway-production"
type = "railway"
project_id = "project-id"
environment_id = "environment-id"
service_id = "service-id"
token = { env = "RAILWAY_TOKEN" }
auth = "project_token"
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["api"],
            Backend::Railway {
                environment_id,
                scope: RailwayScope::Service,
                service_id,
                token: StringValue::Source { env: Some(token_env), op: None },
                auth: RailwayAuth::ProjectToken,
            } if matches!(environment_id, StringValue::Literal(value) if value == "environment-id")
                && matches!(service_id, Some(StringValue::Literal(value)) if value == "service-id")
                && token_env == "RAILWAY_TOKEN"
        ));
    }

    #[test]
    fn parses_optional_victoria_logs_scope_filter() {
        let config: Config = toml::from_str(
            r#"[projects.scoped]
name = "scoped-production"
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN" }
scope_filter = "_stream:{environment=\"production\"}"

[projects.unscoped]
name = "unscoped-production"
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN" }
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["scoped"],
            Backend::VictoriaLogs {
                scope_filter: Some(StringValue::Literal(scope_filter)),
                ..
            } if scope_filter == "_stream:{environment=\"production\"}"
        ));
        assert!(matches!(
            &config.projects["unscoped"],
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
name = "railway-production"
type = "railway"
environment_id = "environment-id"
scope = "environment"
token = { env = "RAILWAY_TOKEN" }
auth = "bearer"
"#,
        )
        .unwrap();

        assert!(matches!(
            &config.projects["api"],
            Backend::Railway {
                environment_id,
                scope: RailwayScope::Environment,
                service_id: None,
                auth: RailwayAuth::Bearer,
                ..
            } if matches!(environment_id, StringValue::Literal(value) if value == "environment-id")
        ));
    }

    #[test]
    fn resolves_literal_string_without_reinterpreting_it() {
        let value: TestValue =
            toml::from_str(r#"value = "op://production/grafana/token""#).unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| panic!("literal read environment"),
                |_| panic!("literal read op"),
            )
            .unwrap();

        assert_eq!(resolved, "op://production/grafana/token");
    }

    #[test]
    fn resolves_env_only_string() {
        let value: TestValue = toml::from_str(r#"value = { env = "GRAFANA_TOKEN" }"#).unwrap();

        let resolved = value
            .value
            .resolve_with(
                |name| {
                    assert_eq!(name, "GRAFANA_TOKEN");
                    Ok("environment-secret".to_owned())
                },
                |_| panic!("env-only value read op"),
            )
            .unwrap();

        assert_eq!(resolved, "environment-secret");
    }

    #[test]
    fn resolves_op_only_string() {
        let value: TestValue =
            toml::from_str(r#"value = { op = "op://production/grafana/token" }"#).unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| panic!("op-only value read environment"),
                |reference| {
                    assert_eq!(reference, "op://production/grafana/token");
                    Ok("one-password-secret".to_owned())
                },
            )
            .unwrap();

        assert_eq!(resolved, "one-password-secret");
    }

    #[test]
    fn resolves_environment_before_op() {
        let value: TestValue = toml::from_str(
            r#"value = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }"#,
        )
        .unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| Ok("environment-secret".to_owned()),
                |_| panic!("op fallback ran despite environment value"),
            )
            .unwrap();

        assert_eq!(resolved, "environment-secret");
    }
}
