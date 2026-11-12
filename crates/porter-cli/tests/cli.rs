use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;

fn porter(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("porter").unwrap();
    c.arg("--project").arg(dir).timeout(Duration::from_secs(5));
    c
}

fn init(dir: &std::path::Path) {
    porter(dir)
        .args(["init", "demo", "--adapter", "wordpress"])
        .write_stdin("")
        .assert()
        .success();
}

#[test]
fn json_stdout_is_pure_ndjson() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let out = porter(d.path())
        .args(["--json", "status"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).expect("every stdout line is JSON"))
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "status");
    assert_eq!(lines[0]["v"], 1);
}

#[test]
fn json_errors_keep_stdout_empty() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path())
        .args(["--json", "analyse"])
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn missing_config_exits_1_naming_key() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("porter.toml"),
        "[project]\nname = \"x\"\n\n[source]\n",
    )
    .unwrap();
    porter(d.path())
        .arg("analyse")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("source.adapter"));
}

#[test]
fn unregistered_adapter_exits_1_naming_it() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path())
        .arg("analyse")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "no source adapter named \"wordpress\" (available: none)",
        ));
}

#[test]
fn no_prompt_without_tty() {
    let d = tempfile::tempdir().unwrap();
    porter(d.path())
        .args(["init", "x", "--adapter", "wordpress", "--interactive"])
        .write_stdin("")
        .assert()
        .success();
}

#[test]
fn clean_keeps_user_files() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    for f in ["raw", "media", "reports"] {
        std::fs::create_dir_all(d.path().join(f)).unwrap();
        std::fs::write(d.path().join(f).join("x"), "x").unwrap();
    }
    std::fs::write(d.path().join("notes.md"), "mine").unwrap();
    std::fs::create_dir(d.path().join("mine")).unwrap();
    porter(d.path()).arg("clean").assert().success();
    for f in ["raw", "media", "reports", "project.db"] {
        assert!(!d.path().join(f).exists(), "{f} should be gone");
    }
    assert!(d.path().join("porter.toml").exists());
    assert!(d.path().join("notes.md").exists());
    assert!(d.path().join("mine").exists());
}

#[test]
fn old_flags_rejected() {
    Command::cargo_bin("porter")
        .unwrap()
        .args(["--source", "umbraco"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("2.0"));
}

#[test]
fn issues_lists_and_resolves() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path())
        .args(["--json", "issues", "--open"])
        .assert()
        .success()
        .stdout("");
    porter(d.path())
        .args(["issues", "--resolve", "nope", "--as", "wont-fix"])
        .assert()
        .code(1);
}

#[test]
fn verify_without_destination_exits_1() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path()).arg("verify").assert().code(1);
}

#[test]
fn export_writes_file() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let f = d.path().join("out.json");
    porter(d.path())
        .arg("export")
        .arg("--json")
        .arg(&f)
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
    assert_eq!(v["v"], 1);
    assert!(v["issues"].is_array());
}
