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
    // The envelope leads, as in every other event.
    assert!(
        stdout.starts_with(r#"{"v":1,"event":"status","#),
        "{stdout}"
    );
}

#[test]
fn json_errors_keep_stdout_empty() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("porter.toml"),
        "[project]\nname = \"demo\"\n\n[source]\nadapter = \"nope\"\n",
    )
    .unwrap();
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
    std::fs::write(
        d.path().join("porter.toml"),
        "[project]\nname = \"demo\"\n\n[source]\nadapter = \"nope\"\n",
    )
    .unwrap();
    porter(d.path())
        .arg("analyse")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "no source adapter named \"nope\" (available: wordpress, umbraco, sanity)",
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

#[test]
fn analyse_wordpress_wxr_end_to_end() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let wxr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(&format!(
        "\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
        wxr.display().to_string()
    ));
    std::fs::write(&toml, text).unwrap();
    porter(d.path())
        .args(["--json", "analyse"])
        .assert()
        .success();
    assert!(d.path().join("raw/analysis/coverage.json").exists());
    let seeded = std::fs::read_to_string(&toml).unwrap();
    assert!(seeded.contains("[mapping"), "{seeded}");
    porter(d.path()).arg("report").assert().success();
    assert!(d.path().join("reports/analysis.html").exists());
}

#[test]
fn analyse_seeds_mapping_once_and_reports_it() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let wxr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(&format!(
        "\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
        wxr.display().to_string()
    ));
    std::fs::write(&toml, text).unwrap();
    porter(d.path())
        .arg("analyse")
        .assert()
        .success()
        .stderr(predicate::str::contains("mapping seeded in porter.toml"));
    let after = std::fs::read_to_string(&toml).unwrap();
    assert!(after.contains("[mapping.types."), "{after}");
    // Second run never overwrites.
    porter(d.path())
        .arg("analyse")
        .assert()
        .success()
        .stderr(predicate::str::contains("mapping seeded").not());
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), after);
    // The seeded file still loads.
    porter(d.path()).arg("status").assert().success();
    // Under --json the notice is an NDJSON event, on a fresh project.
    let d2 = tempfile::tempdir().unwrap();
    init(d2.path());
    let toml2 = d2.path().join("porter.toml");
    let mut t2 = std::fs::read_to_string(&toml2).unwrap();
    t2.push_str(&format!(
        "\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
        wxr.display().to_string()
    ));
    std::fs::write(&toml2, t2).unwrap();
    let out = porter(d2.path())
        .args(["--json", "analyse"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.lines().any(|l| l.contains("\"mapping-seeded\"")));
}

#[test]
fn normalise_wordpress_wxr_end_to_end() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let wxr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(&format!(
        "\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
        wxr.display().to_string()
    ));
    std::fs::write(&toml, &text).unwrap();
    for stage in ["analyse", "extract"] {
        porter(d.path()).arg(stage).assert().success();
    }
    let code = porter(d.path())
        .args(["--json", "normalise"])
        .assert()
        .get_output()
        .status
        .code();
    assert!(matches!(code, Some(0 | 2)), "normalise exited {code:?}");
    let out = porter(d.path())
        .args(["--json", "status"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let status: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let n = &status["stages"]["normalise"];
    assert!(n["done"].as_u64().unwrap_or(0) >= 2, "{status}");
    // The stage views the desktop and TUI show, from the engine's `stage_view`.
    let views = &status["stage_views"];
    assert!(
        matches!(
            views["normalise"]["state"].as_str(),
            Some("clean" | "warnings" | "errors")
        ),
        "{status}"
    );
    assert_eq!(views["normalise"]["stale"], serde_json::Value::Null);
    assert_eq!(views["load"]["state"], "not-run");
    assert_eq!(status["ever_written"], false);
    // A porter.toml section a stage read changes: that stage reads stale, with why.
    std::fs::write(
        &toml,
        format!("{text}\n[[redirects]]\nfrom = \"/a\"\nto = \"/b\"\n"),
    )
    .unwrap();
    let out = porter(d.path())
        .args(["--json", "status"])
        .assert()
        .success();
    let status: serde_json::Value =
        serde_json::from_slice(out.get_output().stdout.trim_ascii()).unwrap();
    assert_eq!(
        status["stage_views"]["normalise"]["stale"], "redirects changed since",
        "{status}"
    );
    assert_eq!(
        status["stage_views"]["extract"]["stale"],
        serde_json::Value::Null
    );
    let out = porter(d.path()).arg("status").assert().success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(text.contains("state"), "{text}");
    assert!(
        text.contains("(stale)") && text.contains("↻ redirects changed since"),
        "{text}"
    );
    assert!(text.contains("not run"), "{text}");
}

#[test]
fn normalise_umbraco_json_end_to_end() {
    let d = tempfile::tempdir().unwrap();
    porter(d.path())
        .args(["init", "demo", "--adapter", "umbraco"])
        .write_stdin("")
        .assert()
        .success();
    let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/umbraco/sample.json");
    let toml = d.path().join("porter.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(&format!(
        "\n[[connectors]]\nkind = \"umbraco-json\"\npaths = [{:?}]\n",
        sample.display().to_string()
    ));
    std::fs::write(&toml, text).unwrap();
    for stage in ["analyse", "extract"] {
        let code = porter(d.path())
            .arg(stage)
            .assert()
            .get_output()
            .status
            .code();
        assert!(matches!(code, Some(0 | 2)), "{stage} exited {code:?}");
    }
    assert!(d.path().join("raw/umbraco/export-0.json").exists());
    let code = porter(d.path())
        .args(["--json", "normalise"])
        .assert()
        .get_output()
        .status
        .code();
    assert!(matches!(code, Some(0 | 2)), "normalise exited {code:?}");
    let out = porter(d.path())
        .args(["--json", "status"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("normalise"), "{stdout}");
}

#[test]
fn upgrade_args_parse() {
    Command::cargo_bin("porter")
        .unwrap()
        .args(["upgrade", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--check").and(predicate::str::contains("--yes")));
}

#[test]
fn init_rejects_unknown_adapter() {
    let d = tempfile::tempdir().unwrap();
    porter(d.path())
        .args(["init", "x", "--adapter", "nope"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "unknown adapter \"nope\" (available: wordpress, umbraco, sanity)",
        ));
    assert!(!d.path().join("porter.toml").exists());
}

#[test]
fn dry_run_run_skips_verify() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let wxr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let text = std::fs::read_to_string(&toml)
        .unwrap()
        .replace("[destination]\n", "[destination]\nadapter = \"payload\"\nurl = \"http://127.0.0.1:9\"\napi_key_env = \"PORTER_TEST_UNSET_KEY\"\n");
    std::fs::write(
        &toml,
        format!(
            "{text}\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
            wxr.display().to_string()
        ),
    )
    .unwrap();
    let out = porter(d.path())
        .env_remove("PORTER_TEST_UNSET_KEY")
        .timeout(Duration::from_secs(30))
        .args(["--json", "run", "--continue"])
        .assert();
    let o = out.get_output();
    let stdout = String::from_utf8(o.stdout.clone()).unwrap();
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "stage-finished" && e["stage"] == "load"),
        "{stdout}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e["event"] == "stage-started" && e["stage"] == "verify"),
        "{stdout}"
    );
    assert!(
        events.iter().any(|e| e["event"] == "stage-skipped"
            && e["stage"] == "verify"
            && e["reason"] == "dry run"),
        "{stdout}"
    );
    // Verify never asked for the (unset) API key.
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!stderr.contains("PORTER_TEST_UNSET_KEY"), "{stderr}");
}

#[test]
fn dry_run_load_says_so_in_the_summary() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let toml = d.path().join("porter.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace(
        "[destination]\n",
        "[destination]\nadapter = \"payload\"\nurl = \"http://127.0.0.1:9\"\napi_key_env = \"PORTER_TEST_UNSET_KEY\"\n",
    );
    std::fs::write(&toml, text).unwrap();
    let out = porter(d.path()).arg("load").assert();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("load (dry run)"), "{stdout}");
}

#[test]
fn init_sanity_project() {
    let d = tempfile::tempdir().unwrap();
    porter(d.path())
        .args(["init", "kirker", "--adapter", "sanity"])
        .write_stdin("")
        .assert()
        .success();
    let t = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
    assert!(t.contains("adapter = \"sanity\""), "{t}");
    assert!(t.contains("# kind = \"sanity-api\""), "{t}");
}

/// A project whose mapping sends `post` to `pages`, and a repo for `--into`.
fn into_fixture(slug: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let toml = std::fs::read_to_string(d.path().join("porter.toml"))
        .unwrap()
        .replace("# adapter = \"\"", "adapter = \"payload\"")
        .replace(
            "# [mapping]",
            &format!("[mapping.types.post]\nto = \"{slug}\"\n"),
        );
    std::fs::write(d.path().join("porter.toml"), toml).unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join("src/collections")).unwrap();
    std::fs::write(
        repo.path().join("src/collections/Pages.ts"),
        "export const Pages = { slug: \"pages\", fields: [] }\n",
    )
    .unwrap();
    (d, repo)
}

#[test]
fn schema_generate_into_collision_exits_2_and_writes_nothing() {
    let (d, repo) = into_fixture("pages");
    porter(d.path())
        .args(["schema", "generate", "--into"])
        .arg(repo.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "collection `pages` is already declared",
        ));
    assert!(!repo.path().join("src/collections/porter").exists());
}

#[test]
fn schema_generate_into_prints_config_lines_and_route() {
    let (d, repo) = into_fixture("articles");
    porter(d.path())
        .args(["schema", "generate", "--into"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "import { porterCollections } from './collections/porter'",
        ))
        .stdout(predicate::str::contains("...porterCollections"))
        // Optional catch-all: no segments is the home page, `/`.
        .stdout(predicate::str::contains("params).path ?? []"))
        .stdout(predicate::str::contains(
            "app/(frontend)/[[...path]]/page.tsx",
        ));
    assert!(
        repo.path()
            .join("src/collections/porter/Articles.ts")
            .is_file()
    );
    assert!(
        repo.path()
            .join("src/collections/porter/index.ts")
            .is_file()
    );
}

#[test]
fn into_needs_generate() {
    let (d, repo) = into_fixture("articles");
    porter(d.path())
        .args(["schema", "check", "--into"])
        .arg(repo.path())
        .assert()
        .code(2);
}

/// A Sanity project over the engine's fixture export, analysed and normalised.
fn normalised_sanity() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    porter(d.path())
        .args(["init", "kirker", "--adapter", "sanity"])
        .write_stdin("")
        .assert()
        .success();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/sanity");
    let f = d.path().join("porter.toml");
    let mut t = std::fs::read_to_string(&f).unwrap();
    t.push_str(&format!(
        "\n[[connectors]]\nkind = \"sanity-export\"\npath = \"{}\"\n",
        fixture.display()
    ));
    std::fs::write(&f, t).unwrap();
    for stage in ["analyse", "normalise"] {
        porter(d.path())
            .arg(stage)
            .assert()
            .code(predicate::in_iter([0, 2]));
    }
    d
}

fn tidy_list(d: &std::path::Path) -> Vec<serde_json::Value> {
    let out = porter(d).args(["--json", "tidy", "list"]).output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn tidy_suggest_needs_normalised_documents() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path())
        .args(["tidy", "suggest"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("run porter normalise first"));
}

#[test]
fn tidy_enable_round_trip() {
    let d = normalised_sanity();
    // normalise already appended the suggestions (off); `suggest` has nothing new.
    let rules = tidy_list(d.path());
    let pages = rules.iter().find(|r| r["id"] == "pages").unwrap();
    assert_eq!(
        pages["label"],
        "Merge Country pages and Region pages into Pages"
    );
    assert_eq!(pages["event"], "tidy");
    assert_eq!(pages["v"], 1);
    assert_eq!(pages["enabled"], false);
    porter(d.path())
        .args(["tidy", "enable", "pages"])
        .assert()
        .success();
    let rules = tidy_list(d.path());
    assert_eq!(
        rules.iter().find(|r| r["id"] == "pages").unwrap()["enabled"],
        true
    );
    porter(d.path())
        .args(["tidy", "disable", "pages"])
        .assert()
        .success();
    assert_eq!(
        tidy_list(d.path())
            .iter()
            .find(|r| r["id"] == "pages")
            .unwrap()["enabled"],
        false
    );
    // Suggesting again adds nothing, and the file's comments survive all of it.
    porter(d.path())
        .args(["--json", "tidy", "suggest"])
        .assert()
        .success()
        .stdout("");
    let t = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
    assert!(t.contains("# kind = \"sanity-api\""), "{t}");
    assert!(t.contains("# Seeded by `porter analyse`"), "{t}");
}

#[test]
fn tidy_enable_unknown_id_exits_1() {
    let d = normalised_sanity();
    porter(d.path())
        .args(["tidy", "enable", "nope"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("known: ").and(predicate::str::contains("pages")));
}

#[test]
fn flow_needs_normalise() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    porter(d.path())
        .arg("flow")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("run porter normalise first"));
}

#[test]
fn flow_json_is_one_line_and_normalise_wrote_it() {
    let d = normalised_sanity();
    assert!(d.path().join("reports/flow.json").exists());
    let out = porter(d.path()).args(["--json", "flow"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert_eq!(s.lines().count(), 1, "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(
        (v["v"].as_u64(), v["event"].as_str()),
        (Some(1), Some("flow"))
    );
    assert!(v["graph"]["counted_at"].is_null());
    // Suggested rules appear as disabled cards.
    let pages = v["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "tidy:pages")
        .unwrap();
    assert_eq!(pages["enabled"], false);
    porter(d.path())
        .arg("flow")
        .assert()
        .success()
        .stdout(predicate::str::contains("Tidy-ups").and(predicate::str::contains("not counted")));
}
