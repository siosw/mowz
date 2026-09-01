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
    op_account: Option<String>,
}

impl StringValue {
    pub(crate) fn resolve(&self) -> Result<String> {
        self.resolve_with(|name| env::var(name), read_op)
    }

    fn resolve_with<E, O>(&self, mut read_env: E, mut read_op: O) -> Result<String>
    where
        E: FnMut(&str) -> std::result::Result<String, env::VarError>,
        O: FnMut(&str, Option<&str>) -> Result<String>,
    {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Source(StringSource {
                env,
                op,
                op_account,
            }) => {
                if op.is_none() && op_account.is_some() {
                    bail!("string source cannot configure `op_account` without `op`");
                }

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
                    return read_op(reference, op_account.as_deref());
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

fn read_op(reference: &str, account: Option<&str>) -> Result<String> {
    let output = op_read_command(reference, account)
        .output()
        .wrap_err(
            "failed to run `op read`; install the 1Password CLI and authenticate it, or provide the configured environment variable",
        )?;

    parse_op_output(output)
}

fn parse_op_output(output: std::process::Output) -> Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let message = "`op read` failed; check that the 1Password CLI is authenticated and the configured reference is accessible";
        if stderr.is_empty() {
            bail!(message);
        }
        bail!("{message}: {stderr}");
    }

    String::from_utf8(output.stdout)
        .wrap_err("`op read` returned a configured value that is not valid UTF-8")
}

fn op_read_command(reference: &str, account: Option<&str>) -> Command {
    let mut command = Command::new("op");
    command.args(["read", "--no-newline"]);
    if let Some(account) = account {
        command.args(["--account", account]);
    }
    command.arg(reference);
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
                |_, _| panic!("literal read op"),
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
                |_, _| panic!("env-only value read op"),
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
                |reference, account| {
                    assert_eq!(reference, "op://production/grafana/token");
                    assert_eq!(account, None);
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
                |_, _| panic!("op fallback ran despite environment value"),
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
                |reference, account| {
                    assert_eq!(reference, "op://production/grafana/token");
                    assert_eq!(account, None);
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
                |_, _| panic!("op fallback ran despite an empty environment value"),
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
                |_, _| panic!("resolved environment value was reinterpreted"),
            )
            .unwrap();

        assert_eq!(resolved, "op://production/grafana/token");
    }

    #[test]
    fn resolves_op_with_configured_account() {
        let value: TestValue = toml::from_str(
            r#"value = { op = "op://Private/DIALECTIC_GRAFANA_TOKEN/credential", op_account = "54BDP35LLRDPFBNWXFDQQYCVXU" }"#,
        )
        .unwrap();

        let resolved = value
            .value
            .resolve_with(
                |_| panic!("op-only value read environment"),
                |reference, account| {
                    assert_eq!(reference, "op://Private/DIALECTIC_GRAFANA_TOKEN/credential");
                    assert_eq!(account, Some("54BDP35LLRDPFBNWXFDQQYCVXU"));
                    Ok("one-password-secret".to_owned())
                },
            )
            .unwrap();

        assert_eq!(resolved, "one-password-secret");
    }

    #[test]
    fn rejects_op_account_without_op() {
        let value: TestValue = toml::from_str(
            r#"value = { env = "GRAFANA_TOKEN", op_account = "54BDP35LLRDPFBNWXFDQQYCVXU" }"#,
        )
        .unwrap();

        let error = value
            .value
            .resolve_with(
                |_| panic!("invalid source read environment"),
                |_, _| panic!("invalid source read op"),
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot configure `op_account` without `op`"),
            "{error}"
        );
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
    fn builds_op_read_command_without_account() {
        let command = op_read_command("op://production/grafana/token", None);

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

    #[test]
    fn builds_op_read_command_with_account() {
        let command = op_read_command(
            "op://Private/DIALECTIC_GRAFANA_TOKEN/credential",
            Some("54BDP35LLRDPFBNWXFDQQYCVXU"),
        );

        assert_eq!(command.get_program(), OsStr::new("op"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("read"),
                OsStr::new("--no-newline"),
                OsStr::new("--account"),
                OsStr::new("54BDP35LLRDPFBNWXFDQQYCVXU"),
                OsStr::new("op://Private/DIALECTIC_GRAFANA_TOKEN/credential"),
            ]
        );
    }

    #[test]
    fn includes_trimmed_op_stderr_without_stdout_on_failure() {
        let mut output = Command::new("sh")
            .args([
                "-c",
                "printf '  account is ambiguous  \\n' >&2; printf 'secret-value'; exit 1",
            ])
            .output()
            .unwrap();
        output.stdout = b"sentinel-secret-value".to_vec();

        let error = parse_op_output(output).unwrap_err().to_string();

        assert!(error.ends_with(": account is ambiguous"), "{error}");
        assert!(!error.contains("sentinel-secret-value"), "{error}");
    }
}
