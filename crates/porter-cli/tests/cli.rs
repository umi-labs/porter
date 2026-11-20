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

/// A project whose mapping sends `post` to `slug`, and a repo for `--into` declaring `pages`.
fn into_fixture(slug: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    into_fixture_with(&format!("[mapping.types.post]\nto = \"{slug}\"\n"))
}

fn into_fixture_with(mapping: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    let toml = std::fs::read_to_string(d.path().join("porter.toml"))
        .unwrap()
        .replace("# adapter = \"\"", "adapter = \"payload\"")
        .replace("# [mapping]", mapping);
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

/// A type mapped onto the app's own collection on purpose: that collection isn't
/// generated (the app has it), the rest is, and the report says so (R20).
#[test]
fn schema_generate_into_skips_a_collection_mapped_onto_the_app() {
    let (d, repo) = into_fixture_with(
        "[mapping.types.post]\nto = \"pages\"\n\n[mapping.types.event]\nto = \"events\"\n",
    );
    porter(d.path())
        .args(["schema", "generate", "--into"])
        .arg(repo.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "`post` maps onto existing collection `pages` (src/collections/Pages.ts); not generated: run porter schema check for missing fields",
        ));
    let porter_dir = repo.path().join("src/collections/porter");
    assert!(!porter_dir.join("Pages.ts").exists());
    assert!(porter_dir.join("Events.ts").is_file());
    let index = std::fs::read_to_string(porter_dir.join("index.ts")).unwrap();
    assert!(index.contains("[Events]"), "{index}");
}

/// A block slug or interface name the app already declares stops everything (R20).
#[test]
fn schema_generate_into_block_collision_writes_nothing() {
    let (d, repo) = into_fixture_with(
        "[mapping.types.post]\nto = \"articles\"\n\n[[mapping.blocks]]\nmatch = { kind = \"hero\" }\nto = \"hero\"\n",
    );
    std::fs::create_dir_all(repo.path().join("src/blocks/Hero")).unwrap();
    std::fs::write(
        repo.path().join("src/blocks/Hero/config.ts"),
        "export const Hero = { slug: 'hero', interfaceName: 'HeroBlock', fields: [] }\n",
    )
    .unwrap();
    porter(d.path())
        .args(["schema", "generate", "--into"])
        .arg(repo.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "block `hero` is already declared in src/blocks/Hero/config.ts; nothing written",
        ));
    assert!(!repo.path().join("src/collections/porter").exists());
}

/// A collection porter would generate under a name the app already has, with no type
/// mapped onto it by design (the seed's `pages` -> `pages`), stops everything.
#[test]
fn schema_generate_into_collision_exits_2_and_writes_nothing() {
    let (d, repo) = into_fixture_with("[mapping.types.pages]\nto = \"pages\"\n");
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

/// R26: a dismissed suggestion stays in porter.toml (so it isn't suggested again), off,
/// and out of the flow.
#[test]
fn tidy_dismiss_keeps_the_rule_out_of_suggestions_and_flow() {
    let d = normalised_sanity();
    porter(d.path())
        .args(["tidy", "enable", "pages"])
        .assert()
        .success();
    porter(d.path())
        .args(["tidy", "dismiss", "pages"])
        .assert()
        .success();
    let rules = tidy_list(d.path());
    let pages = rules.iter().find(|r| r["id"] == "pages").unwrap();
    assert_eq!(
        (&pages["enabled"], &pages["dismissed"]),
        (&serde_json::json!(false), &serde_json::json!(true))
    );
    let t = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
    assert!(t.contains("dismissed = true"), "{t}");
    // Not suggested again, by `suggest` or the next normalise.
    porter(d.path())
        .args(["--json", "tidy", "suggest"])
        .assert()
        .success()
        .stdout("");
    porter(d.path())
        .arg("normalise")
        .assert()
        .code(predicate::in_iter([0, 2]));
    assert_eq!(
        tidy_list(d.path())
            .iter()
            .filter(|r| r["from"] == pages["from"])
            .count(),
        1
    );
    let out = porter(d.path()).args(["--json", "flow"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let nodes = v["graph"]["nodes"].as_array().unwrap();
    assert!(nodes.iter().all(|n| n["id"] != "tidy:pages"), "{nodes:?}");
    assert!(nodes.iter().any(|n| n["id"] == "tidy:card-carousels"));
    porter(d.path())
        .args(["tidy", "dismiss", "nope"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("known: "));
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

// ---- Sanity end to end, against a mock API and CDN ----

const ASSET_PATHS: [&str; 4] = [
    "/images/abc/production/d3dfdcf650653fee4a8b89c828ccad98a8406fdd-800x600.jpg",
    "/images/abc/production/d267af74a142296235941b40d70bc38113a0481e-800x600.jpg",
    "/images/abc/production/365899501b2366f3fc75a025658705cc0f32deba-800x600.jpg",
    "/files/abc/production/ca7fcf72964eb65f1a947189036a170a1489a0bb.pdf",
];

/// `porter` against `dir`, with the Sanity token and API base set on the child only.
fn sanity_porter(dir: &std::path::Path, base: &str) -> Command {
    let mut c = Command::cargo_bin("porter").unwrap();
    c.arg("--project")
        .arg(dir)
        .timeout(Duration::from_secs(60))
        .env("PORTER_SANITY_TOKEN", "t0ken")
        .env("PORTER_SANITY_API_BASE", base);
    c
}

fn sanity_api_project(base: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sanity_porter(d.path(), base)
        .args(["init", "kirker", "--adapter", "sanity"])
        .write_stdin("")
        .assert()
        .success();
    let f = d.path().join("porter.toml");
    let mut t = std::fs::read_to_string(&f).unwrap();
    t.push_str(
        "\n[[connectors]]\nkind = \"sanity-api\"\nproject_id = \"proj1\"\ndataset = \"production\"\n",
    );
    std::fs::write(&f, t).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread")]
async fn sanity_end_to_end_cli() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/sanity/data.ndjson");
    let data = std::fs::read_to_string(fixture)
        .unwrap()
        .replace("http://cdn.test", &server.uri());
    Mock::given(method("GET"))
        .and(path("/v2021-06-07/data/export/production"))
        .respond_with(ResponseTemplate::new(200).set_body_string(data))
        .mount(&server)
        .await;
    for (i, p) in ASSET_PATHS.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(*p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(format!("asset-{}", i + 1)))
            .mount(&server)
            .await;
    }
    let base = server.uri();
    let d = sanity_api_project(&base);
    let ok = predicate::in_iter([0, 2]);
    let run = |stage: &str| {
        sanity_porter(d.path(), &base)
            .arg(stage)
            .assert()
            .code(ok.clone());
    };
    run("analyse");
    // `enquiryForm` has 3 documents, under the detection threshold of 50, so list it. This
    // goes in after analyse, which seeds an empty `[mapping.forms]` only while the file has
    // no `[mapping]`.
    let f = d.path().join("porter.toml");
    let t = std::fs::read_to_string(&f).unwrap().replace(
        "[mapping.forms]\n",
        "[mapping.forms]\nsubmission_types = [\"enquiryForm\"]\n",
    );
    std::fs::write(&f, t).unwrap();
    run("extract");
    run("normalise");
    // The suggested merge of the page types: its id isn't fixed, so read it from the list.
    let out = sanity_porter(d.path(), &base)
        .args(["--json", "tidy", "list"])
        .output()
        .unwrap();
    let rules: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let merge = rules
        .iter()
        .find(|r| r["from"] == serde_json::json!(["countryPage", "regionPage"]))
        .expect("a merge-types suggestion over the page types");
    let id = merge["id"].as_str().unwrap().to_string();
    assert_eq!(merge["enabled"], false);
    sanity_porter(d.path(), &base)
        .args(["tidy", "enable", &id])
        .assert()
        .success();
    let out = sanity_porter(d.path(), &base)
        .args(["--json", "flow"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let node = v["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == format!("tidy:{id}"))
        .expect("the rule's flow node");
    assert_eq!(node["enabled"], true);
    // The submission type is counted, never shown.
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains("person1@") && !s.contains("Person 1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sanity_401_exits_1_naming_the_token_var() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let base = server.uri();
    let d = sanity_api_project(&base);
    sanity_porter(d.path(), &base)
        .arg("analyse")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("PORTER_SANITY_TOKEN"));
}

/// The documented user path, nothing pre-seeded: init → analyse (seeds `[mapping]`) →
/// extract → normalise → schema generate → dry-run load. A straight move must carry the
/// content fields all the way to the preview bodies (C1).
#[test]
fn sanity_straight_move_carries_content_fields() {
    let d = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("porter").unwrap();
        c.arg("--project")
            .arg(d.path())
            .args(args)
            .timeout(Duration::from_secs(60))
            .assert()
    };
    run(&["init", "kirker", "--adapter", "sanity"]).success();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../porter-engine/tests/fixtures/sanity");
    let f = d.path().join("porter.toml");
    let t = std::fs::read_to_string(&f)
        .unwrap()
        .replace("# adapter = \"\"", "adapter = \"payload\"")
        .replace(
            "# url = \"https://cms.example.com\"",
            "url = \"http://127.0.0.1:9\"",
        )
        .replace(
            "# api_key_env = \"MY_API_KEY_ENV_VAR\"",
            "api_key_env = \"PORTER_TEST_UNSET_KEY\"",
        )
        + &format!(
            "\n[[connectors]]\nkind = \"sanity-export\"\npath = \"{}\"\n",
            fixture.display()
        );
    std::fs::write(&f, t).unwrap();
    for stage in ["analyse", "extract", "normalise"] {
        run(&[stage]).code(predicate::in_iter([0, 2]));
    }
    let toml = std::fs::read_to_string(&f).unwrap();
    assert!(toml.contains("[mapping.types.hotelBreak.fields]"), "{toml}");
    run(&["schema", "generate"]).code(predicate::in_iter([0, 2]));
    let ts = std::fs::read_to_string(d.path().join("generated/payload/HotelBreak.ts")).unwrap();
    for field in ["description", "highlights", "mainImage"] {
        assert!(ts.contains(&format!("name: \"{field}\"")), "{field}: {ts}");
    }
    run(&["load"]).code(predicate::in_iter([0, 2]));
    let dir = d.path().join("raw/load-preview/hotelBreak");
    let bodies: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| {
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(bodies.len(), 3);
    for v in &bodies {
        for field in ["description", "seo", "flexibleContent"] {
            assert!(
                v.get(field).is_some_and(|x| !x.is_null()),
                "{field} not sent: {v}"
            );
        }
    }
    assert!(
        bodies.iter().any(|v| v["highlights"].is_array()),
        "{bodies:?}"
    );
}
