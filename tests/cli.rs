use std::process::Command;

#[test]
fn cli_prints_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: mowz [OPTIONS] <PROJECT> <QUERY>"));
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
    for arguments in [vec!["api"], vec!["api", "query", "extra"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_mowz"))
            .args(arguments)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("<PROJECT> <QUERY>"));
    }
}
