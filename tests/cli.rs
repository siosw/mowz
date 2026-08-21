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
    assert!(stdout.contains("Query a configured project's logs"));
    assert!(stdout.contains("List configured projects"));
    assert!(stdout.contains("Print the mowz Agent Skill"));
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
    assert!(stdout.contains("--limit <LIMIT>"));
    assert!(stdout.contains("[default: 3]"));
}

#[test]
fn cli_prints_the_standard_skill_without_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("skill")
        .current_dir(directory.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, include_bytes!("../skills/mowz/SKILL.md"));
}

#[test]
fn query_accepts_skill_as_a_project_name() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join(".mowz.toml"), "[projects]\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .args(["query", "skill", "error"])
        .current_dir(directory.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("project \"skill\" is not configured")
    );
}

#[test]
fn cli_prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("mowz {}\n", env!("CARGO_PKG_VERSION"))
    );
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

#[test]
fn projects_discovers_config_in_parent_directory() {
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("project").join("src");
    fs::create_dir_all(&child).unwrap();
    fs::write(
        directory.path().join(".mowz.toml"),
        r#"[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = "unused"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("projects")
        .current_dir(child)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"project\":\"api\",\"backend\":\"victoria_logs\"}\n"
    );
}

#[cfg(unix)]
#[test]
fn projects_does_not_load_config_from_symlinked_home_boundary() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let child = home.join("project");
    let home_alias = directory.path().join("home-alias");
    fs::create_dir_all(&child).unwrap();
    fs::write(home.join(".mowz.toml"), "[projects]\n").unwrap();
    symlink(&home, &home_alias).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("projects")
        .current_dir(home_alias.join("project"))
        .env("HOME", home_alias)
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not find .mowz.toml"));
}

#[test]
fn cli_rejects_limits_outside_the_supported_range() {
    for limit in ["0", "101"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
            .args(["query", "--limit", limit, "api", "query"])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
    }
}
