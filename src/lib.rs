mod config_value;
mod grafana;
mod http;
mod railway;
mod time_range;

use std::{collections::BTreeMap, fs, path::Path};

use config_value::StringValue;
use eyre::{Context, Result, bail};
use railway::{RailwayAuth, RailwayScope};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value};
pub use time_range::TimeRange;

pub const MAX_RESULT_LIMIT: usize = 100;

struct QueryOptions<'a> {
    time_range: &'a TimeRange,
    limit: usize,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    projects: BTreeMap<String, Backend>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read {}", path.display()))?;
        let config: Self = toml::from_str(&contents)
            .wrap_err_with(|| format!("failed to parse {}", path.display()))?;
        config
            .validate()
            .wrap_err_with(|| format!("invalid configuration in {}", path.display()))?;
        Ok(config)
    }

    pub fn projects(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.projects
            .iter()
            .map(|(name, backend)| (name.as_str(), backend.name()))
    }

    fn validate(&self) -> Result<()> {
        for (name, backend) in &self.projects {
            backend
                .validate()
                .wrap_err_with(|| format!("invalid project {name:?}"))?;
        }
        Ok(())
    }
}

impl Backend {
    fn name(&self) -> &'static str {
        match self {
            Self::VictoriaLogs { .. } => "victoria_logs",
            Self::Railway { .. } => "railway",
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::VictoriaLogs {
                url,
                datasource_uid,
                token,
                scope_filter,
            } => {
                url.validate().wrap_err("invalid `url`")?;
                datasource_uid
                    .validate()
                    .wrap_err("invalid `datasource_uid`")?;
                token.validate().wrap_err("invalid `token`")?;
                if let Some(scope_filter) = scope_filter {
                    scope_filter.validate().wrap_err("invalid `scope_filter`")?;
                }
            }
            Self::Railway {
                environment_id,
                scope,
                service_id,
                token,
                ..
            } => {
                scope.validate(service_id.is_some())?;
                environment_id
                    .validate()
                    .wrap_err("invalid `environment_id`")?;
                if let Some(service_id) = service_id {
                    service_id.validate().wrap_err("invalid `service_id`")?;
                }
                token.validate().wrap_err("invalid `token`")?;
            }
        }
        Ok(())
    }

    async fn query(
        &self,
        query: &str,
        options: &QueryOptions<'_>,
        client: &Client,
    ) -> Result<Vec<Map<String, Value>>> {
        match self {
            Self::VictoriaLogs {
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
                    options,
                )
                .await?;
                Ok(bound_entries(
                    grafana::extract_entries(&response),
                    false,
                    options.limit,
                ))
            }
            Self::Railway {
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
                    railway::query_logs(client, &token, *auth, &environment_id, &filter, options)
                        .await?;
                Ok(bound_entries(
                    railway::extract_entries(&response),
                    true,
                    options.limit,
                ))
            }
        }
    }
}

pub async fn query_project(
    config: &Config,
    project_name: &str,
    query: &str,
    time_range: &TimeRange,
    limit: usize,
    client: &Client,
) -> Result<Vec<Map<String, Value>>> {
    if !(1..=MAX_RESULT_LIMIT).contains(&limit) {
        bail!("limit must be between 1 and {MAX_RESULT_LIMIT}");
    }
    let options = QueryOptions { time_range, limit };

    let backend = config
        .projects
        .get(project_name)
        .ok_or_else(|| eyre::eyre!("project {project_name:?} is not configured"))?;

    backend.query(query, &options, client).await
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
    limit: usize,
) -> Vec<Map<String, Value>> {
    if entries.len() > limit && retain_newest {
        entries.drain(..entries.len() - limit);
    } else {
        entries.truncate(limit);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_railway_service_config() {
        let config: Config = toml::from_str(
            r#"[projects.api]
type = "railway"
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
                token: StringValue::Source(_),
                auth: RailwayAuth::ProjectToken,
            } if matches!(environment_id, StringValue::Literal(value) if value == "environment-id")
                && matches!(service_id, Some(StringValue::Literal(value)) if value == "service-id")
        ));
    }

    #[test]
    fn parses_optional_victoria_logs_scope_filter() {
        let config: Config = toml::from_str(
            r#"[projects.scoped]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN" }
scope_filter = "_stream:{environment=\"production\"}"

[projects.unscoped]
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
    fn rejects_invalid_railway_scope_relationships_when_loading_config() {
        for (scope, service_id, expected) in [
            ("service", "", "Railway service scope requires service_id"),
            (
                "environment",
                "service_id = { env = \"MOWZ_TEST_MISSING_SERVICE_ID\" }\n",
                "Railway environment scope must not configure service_id",
            ),
        ] {
            let directory = tempdir().unwrap();
            let path = directory.path().join(".mowz.toml");
            fs::write(
                &path,
                format!(
                    r#"[projects.api]
type = "railway"
environment_id = {{ env = "MOWZ_TEST_MISSING_ENVIRONMENT_ID" }}
scope = "{scope}"
{service_id}token = {{ op = "op://missing/railway/token" }}
auth = "project_token"
"#
                ),
            )
            .unwrap();

            let error = Config::load(&path).unwrap_err();
            let diagnostic = format!("{error:?}");
            assert!(
                diagnostic.contains("invalid project \"api\""),
                "{diagnostic}"
            );
            assert!(diagnostic.contains(expected), "{diagnostic}");
        }
    }

    #[test]
    fn rejects_invalid_secret_source_structure_when_loading_config() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".mowz.toml");
        fs::write(
            &path,
            r#"[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "MOWZ_TEST_MISSING_TOKEN", op_account = "account-id" }
"#,
        )
        .unwrap();

        let error = Config::load(&path).unwrap_err();
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("invalid `token`"), "{diagnostic}");
        assert!(
            diagnostic.contains("cannot configure `op_account` without `op`"),
            "{diagnostic}"
        );
    }

    #[test]
    fn rejects_empty_resolved_token() {
        let error = read_token(&StringValue::Literal(String::new())).unwrap_err();

        assert_eq!(
            error.to_string(),
            "configured token resolved to an empty value"
        );
    }

    #[test]
    fn lists_projects_in_name_order_with_their_backends() {
        let config: Config = toml::from_str(
            r#"[projects.worker]
type = "railway"
environment_id = "environment-id"
service_id = "service-id"
token = { env = "RAILWAY_TOKEN" }
auth = "project_token"

[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN" }
"#,
        )
        .unwrap();

        assert_eq!(
            config.projects().collect::<Vec<_>>(),
            [("api", "victoria_logs"), ("worker", "railway")]
        );
    }
}
