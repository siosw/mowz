use std::{env, process::Command};

use eyre::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum StringValue {
    Literal(String),
    Source(StringSource),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StringSource {
    env: Option<String>,
    op: Option<String>,
}

impl StringValue {
    pub(crate) fn resolve(&self) -> Result<String> {
        self.resolve_with(|name| env::var(name), read_op)
    }

    fn resolve_with<E, O>(&self, mut read_env: E, mut read_op: O) -> Result<String>
    where
        E: FnMut(&str) -> std::result::Result<String, env::VarError>,
        O: FnMut(&str) -> Result<String>,
    {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Source(StringSource { env, op }) => {
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
    let output = op_read_command(reference)
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

fn op_read_command(reference: &str) -> Command {
    let mut command = Command::new("op");
    command.args(["read", "--no-newline", reference]);
    command
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct TestValue {
        value: StringValue,
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

    #[test]
    fn resolves_op_when_environment_is_missing() {
        let value: TestValue = toml::from_str(
            r#"value = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }"#,
        )
        .unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| Err(env::VarError::NotPresent),
                |reference| {
                    assert_eq!(reference, "op://production/grafana/token");
                    Ok("one-password-secret".to_owned())
                },
            )
            .unwrap();

        assert_eq!(resolved, "one-password-secret");
    }

    #[test]
    fn does_not_fall_back_when_environment_is_empty() {
        let value: TestValue = toml::from_str(
            r#"value = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }"#,
        )
        .unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| Ok(String::new()),
                |_| panic!("op fallback ran despite an empty environment value"),
            )
            .unwrap();

        assert!(resolved.is_empty());
    }

    #[test]
    fn does_not_reinterpret_resolved_values() {
        let value: TestValue = toml::from_str(r#"value = { env = "GRAFANA_TOKEN" }"#).unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| Ok("op://production/grafana/token".to_owned()),
                |_| panic!("resolved environment value was reinterpreted"),
            )
            .unwrap();

        assert_eq!(resolved, "op://production/grafana/token");
    }

    #[test]
    fn rejects_unknown_string_source_fields() {
        let error = toml::from_str::<TestValue>(
            r#"value = { env = "GRAFANA_TOKEN", opp = "op://production/grafana/token" }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("opp"), "{error}");
    }

    #[test]
    fn builds_exact_op_read_command() {
        let command = op_read_command("op://production/grafana/token");

        assert_eq!(command.get_program(), OsStr::new("op"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("read"),
                OsStr::new("--no-newline"),
                OsStr::new("op://production/grafana/token"),
            ]
        );
    }
}
