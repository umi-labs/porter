#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use clap::Parser;

#[derive(Parser)]
#[command(name = "porter", version)]
struct Cli {}

fn main() {
    Cli::parse();
}
