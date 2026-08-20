use std::{fs, process::Command};

#[test]
fn cli_prints_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: mowz <COMMAND>"));
    assert!(stdout.contains("query"));
    assert!(stdout.contains("projects"));
}

#[test]
fn query_prints_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .args(["query", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: mowz query [OPTIONS] <PROJECT> <QUERY>"));
    assert!(stdout.contains("--from <FROM>"));
    assert!(stdout.contains("--to <TO>"));
}

#[test]
fn cli_prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "mowz 0.1.0\n");
}

#[test]
fn cli_rejects_the_wrong_number_of_arguments() {
    for arguments in [vec!["query", "api"], vec!["query", "api", "query", "extra"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
            .args(arguments)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("<PROJECT> <QUERY>"));
    }
}

#[test]
fn projects_lists_names_and_backends_without_resolving_secrets() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(".mowz.toml"),
        r#"[projects.worker]
type = "railway"
environment_id = "environment-id"
token = { env = "MOWZ_TEST_MISSING_RAILWAY_TOKEN" }
auth = "project_token"

[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "MOWZ_TEST_MISSING_GRAFANA_TOKEN" }
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("projects")
        .current_dir(directory.path())
        .env_remove("MOWZ_TEST_MISSING_RAILWAY_TOKEN")
        .env_remove("MOWZ_TEST_MISSING_GRAFANA_TOKEN")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"project\":\"api\",\"backend\":\"victoria_logs\"}\n\
{\"project\":\"worker\",\"backend\":\"railway\"}\n"
    );
}
