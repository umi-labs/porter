use crate::output::print_json;
use anyhow::{Result, bail};
use porter_engine::flow::{Column, FlowNode, build};
use porter_model::Stage;
use std::path::Path;

fn counts(n: &FlowNode) -> String {
    match &n.counts {
        Some(c) => {
            let mut s = format!(
                "{} in, {} ok, {} skipped, {} failed",
                c.items_in, c.ok, c.skipped, c.failed
            );
            if c.dropped > 0 {
                s += &format!(", {} fields dropped", c.dropped);
            }
            s
        }
        None => "not counted".into(),
    }
}

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    let project = super::open(dir)?;
    let normalised = project
        .store()
        .runs(Stage::Normalise)?
        .iter()
        .any(|r| r.ended_at.is_some());
    if !normalised {
        bail!("run porter normalise first");
    }
    let g = build(&project, project.config())?;
    if json {
        print_json(&serde_json::json!({"v": 1, "event": "flow", "graph": g}));
        return Ok(0);
    }
    for (col, head) in [
        (Column::Source, "Sources"),
        (Column::Tidy, "Tidy-ups"),
        (Column::Destination, "Destinations"),
    ] {
        println!("{head}");
        for n in g.nodes.iter().filter(|n| n.column == col) {
            let off = if n.enabled { "" } else { " (off)" };
            println!("  {:<34} {}", format!("{}{off}", n.title), counts(n));
        }
    }
    match &g.counted_at {
        Some(t) => println!("Counted by the load that finished at {t}."),
        None => println!("Not counted yet: run `porter load` (a dry run is enough)."),
    }
    Ok(0)
}
