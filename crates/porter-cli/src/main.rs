#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::stage::StageCmd;
use std::path::PathBuf;

const OLD_FLAGS_NOTE: &str =
    "porter 2.0 replaced --source/--target; see `porter --help` and UPGRADING.md";

#[derive(Parser)]
#[command(name = "porter", version, after_help = OLD_FLAGS_NOTE)]
pub struct Cli {
    /// Project folder (holds porter.toml)
    #[arg(long, global = true, default_value = ".")]
    pub project: PathBuf,
    /// NDJSON events on stdout; all human text and logs go to stderr
    #[arg(long, global = true)]
    pub json: bool,
    /// Allow prompts (only when stdin is a TTY)
    #[arg(long, global = true)]
    pub interactive: bool,
    /// More log detail on stderr (-v info, -vv debug, -vvv trace)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create porter.toml and an empty project store
    Init {
        name: String,
        #[arg(long)]
        adapter: String,
    },
    /// Counts per stage, open issues, last runs
    Status,
    /// List or resolve issues
    Issues {
        #[arg(long)]
        stage: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        severity: Option<String>,
        #[arg(long)]
        open: bool,
        /// Issue id to resolve (needs --as)
        #[arg(long)]
        resolve: Option<String>,
        /// handled | wont-fix
        #[arg(long = "as")]
        as_: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete raw/, media/, reports/ and project.db
    Clean,
    Analyse {
        #[arg(long)]
        refresh: bool,
    },
    Extract {
        #[arg(long)]
        refresh: bool,
    },
    Normalise {
        #[arg(long)]
        refresh: bool,
    },
    /// check | generate | prompt | spec
    Schema {
        mode: commands::stage::Mode,
    },
    /// Dry run unless --apply
    Load {
        #[arg(long)]
        apply: bool,
    },
    Verify,
    /// analyse, extract, normalise, schema check, load, verify
    Run {
        #[arg(long = "continue")]
        keep_going: bool,
        #[arg(long)]
        apply: bool,
    },
    Report,
    /// Write status and issues to a file (use with --json)
    Export {
        file: PathBuf,
    },
}

fn parse() -> Cli {
    match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            if e.use_stderr() {
                eprintln!("\nnote: {OLD_FLAGS_NOTE}");
            }
            std::process::exit(e.exit_code());
        }
    }
}

async fn dispatch(cli: Cli) -> Result<i32> {
    use commands::*;
    let (dir, json) = (cli.project.clone(), cli.json);
    match cli.command {
        Command::Init { name, adapter } => init::run(&dir, json, &name, &adapter),
        Command::Status => status::run(&dir, json),
        Command::Issues {
            stage,
            kind,
            severity,
            open,
            resolve,
            as_,
            note,
        } => issues::run(
            &dir,
            json,
            issues::Args {
                stage,
                kind,
                severity,
                open,
                resolve,
                as_,
                note,
            },
        ),
        Command::Clean => clean::run(&dir, json),
        Command::Report => report::run(&dir, json),
        Command::Export { file } => export::run(&dir, json, &file),
        Command::Analyse { refresh } => stage::run(&dir, json, StageCmd::Analyse, refresh).await,
        Command::Extract { refresh } => stage::run(&dir, json, StageCmd::Extract, refresh).await,
        Command::Normalise { refresh } => {
            stage::run(&dir, json, StageCmd::Normalise, refresh).await
        }
        Command::Schema { mode } => stage::run(&dir, json, StageCmd::Schema(mode), false).await,
        Command::Load { apply } => stage::run(&dir, json, StageCmd::Load(apply), false).await,
        Command::Verify => stage::run(&dir, json, StageCmd::Verify, false).await,
        Command::Run { keep_going, apply } => stage::run_all(&dir, json, keep_going, apply).await,
    }
}

#[tokio::main]
async fn main() {
    let cli = parse();
    output::init_logging(cli.verbose);
    let code = match dispatch(cli).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}
