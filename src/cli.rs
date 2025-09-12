use clap::{Parser, ArgAction, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "porter", version, about = "Data migration CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long, default_value = "./seed")]
    pub output: String,
    #[arg(long)]
    pub plugin_dir: Option<String>,
    #[arg(long, action=ArgAction::SetTrue)]
    pub list_adapters: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub interactive: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub dry_run: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub verbose: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub debug: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub fixtures: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a new Porter configuration file
    Init {
        /// Output file path for the configuration
        #[arg(short, long, default_value = "porter.config.toml")]
        output: String,
    },
    /// Show current configuration
    Config {
        /// Configuration file to read
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Generate mappings for collections
    Generate {
        /// Configuration file to use
        #[arg(short, long)]
        config: Option<String>,
        /// Collection name to generate mapping for (if not specified, generates for all)
        #[arg(long)]
        collection: Option<String>,
    },
    /// Generate target field graphs and mapping templates only
    Template {
        /// Configuration file to use
        #[arg(short, long)]
        config: Option<String>,
        /// Collection name to generate template for
        #[arg(long)]
        collection: Option<String>,
    },
    /// Migrate data using existing mappings
    Migrate {
        /// Configuration file to use
        #[arg(short, long)]
        config: Option<String>,
        /// Collection name to migrate (if not specified, migrates all)
        #[arg(long)]
        collection: Option<String>,
    },
    /// Explain the migration process and steps
    Explain {
        /// Configuration file to use for explanation
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Clean generated migration artifacts
    Clean {
        /// Configuration file to use (optional; used to infer migrations dir from output)
        #[arg(short, long)]
        config: Option<String>,
        /// Migrations directory to delete (defaults to parent of output in config)
        #[arg(short = 'd', long)]
        dir: Option<String>,
        /// Also delete porter.config file (porter.config.toml/json)
        #[arg(short = 'f', long, action = ArgAction::SetTrue)]
        full: bool,
    },
}