//! Side effects behind the `l`/`L`/`o`/`e`/`y` keys. The pure `update` only asks for them
//! (see `Cmd`); the runtime runs these.
use base64::{Engine, engine::general_purpose::STANDARD};
use porter_engine::report;
use porter_engine::session::Session;
use std::path::{Path, PathBuf};
use std::process::Command;

/// OSC 52: ask the terminal to put `text` on the system clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

/// `$VISUAL`, else `$EDITOR`, else `vi`.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "vi".into())
}

/// Runs `editor` (a shell word list such as `code -w`) on `path` and waits. The path is
/// a positional argument, never spliced into the script.
pub fn run_editor(editor: &str, path: &Path) -> Result<(), String> {
    let st = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .map_err(|e| format!("could not start editor `{editor}`: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("editor `{editor}` exited with {st}"))
    }
}

/// After the editor: reload `porter.toml` and return what to tell the user (one line
/// per problem, or "config OK").
pub fn reload_after_edit(session: &Session, edited: Result<(), String>) -> Vec<String> {
    let mut msgs: Vec<String> = edited.err().into_iter().collect();
    msgs.extend(session.reload_config());
    if msgs.is_empty() {
        msgs.push("config OK".into());
    }
    msgs
}

/// The report file to open: `<name>.html` if it was written, else analysis, else any.
pub fn pick_report<'a>(paths: &'a [PathBuf], name: &str) -> Option<&'a PathBuf> {
    let named = |n: &str| paths.iter().find(|p| p.file_stem().is_some_and(|s| s == n));
    named(name).or_else(|| named("analysis")).or(paths.first())
}

/// Writes the reports, then opens the most relevant one. Blocking: run off the render loop.
pub fn open_report(session: &Session, name: &str) -> Result<PathBuf, String> {
    let paths = report::write_all(session.project()).map_err(|e| format!("{e:#}"))?;
    let p = pick_report(&paths, name)
        .ok_or("no report yet: run a stage first (analyse writes the first one)")?;
    open::that(p).map_err(|e| format!("could not open {}: {e}", p.display()))?;
    Ok(p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_sequence_is_correct() {
        // base64("abc") = "YWJj"; ESC ] 52 ; c ; <b64> BEL
        assert_eq!(osc52("abc"), "\u{1b}]52;c;YWJj\u{7}");
        assert_eq!(osc52(""), "\u{1b}]52;c;\u{7}");
    }

    #[test]
    fn editor_reload_shows_validation_errors() {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        // The "editor" replaces porter.toml with a config whose URL is not a URL.
        let bad = d.path().join("bad.toml");
        std::fs::write(
            &bad,
            "[project]\nname = \"demo\"\n[source]\nadapter = \"wordpress\"\n[destination]\nadapter = \"payload\"\nurl = \"not a url\"\napi_key_env = \"K\"\n",
        )
        .unwrap();
        let editor = format!("cp '{}'", bad.display());
        let r = run_editor(&editor, &d.path().join("porter.toml"));
        let msgs = reload_after_edit(&s, r);
        assert!(
            msgs.iter().any(|m| m.contains("destination.url")),
            "{msgs:?}"
        );
        // A failing editor is reported, and a clean config says so.
        let msgs = reload_after_edit(&s, run_editor("false", Path::new("x")));
        assert!(msgs[0].contains("exited"), "{msgs:?}");
    }

    #[test]
    fn config_ok_when_nothing_to_say() {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        let ok = d.path().join("ok.toml");
        std::fs::write(
            &ok,
            "[project]\nname = \"demo\"\n[source]\nadapter = \"wordpress\"\n[destination]\nadapter = \"payload\"\nurl = \"https://cms.example.com\"\napi_key_env = \"K\"\n[verify]\nsite_url = \"https://example.com\"\n[[connectors]]\nkind = \"crawl\"\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        run_editor(
            &format!("cp '{}'", ok.display()),
            &d.path().join("porter.toml"),
        )
        .unwrap();
        let msgs = reload_after_edit(&s, Ok(()));
        assert_eq!(msgs, ["config OK"], "{msgs:?}");
    }

    #[test]
    fn report_choice_falls_back_to_analysis() {
        let p = |n: &str| PathBuf::from(format!("reports/{n}.html"));
        let all = [p("analysis"), p("extract")];
        assert_eq!(pick_report(&all, "extract"), Some(&p("extract")));
        assert_eq!(pick_report(&all, "verify"), Some(&p("analysis")));
        assert_eq!(pick_report(&[p("extract")], "verify"), Some(&p("extract")));
        assert_eq!(pick_report(&[], "verify"), None);
    }
}
