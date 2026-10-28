//! The command line: what `pastel` accepts, and running the command it names.
//!
//! `pastel --preview <name>` takes its own set of options, so a command line
//! with `--preview` anywhere in it is parsed separately from the subcommands.
//!
//! The doc comments on the argument types below are the `--help` texts.
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use crate::commands::preview::{self, Background, PreviewArgs};
use crate::commands::{build, package, report, scan, sheets, validate};
use crate::project::Project;

const USAGE: &str = "\
pastel --preview <filename> [options]   recolor one file, show before/after
pastel build [--only CATS] [--clean]    write data/*/updated_graphics
pastel validate                         re-check the output against the source
pastel classify                         show which category every PNG is in
pastel inventory                        list every source file (build/inventory.csv)
pastel sheets [--category CAT]          before/after contact sheets after a build
pastel mod [MOD_SOURCE]                 assemble the Factorio mod in build/mod/
pastel scan <DATA_RAW_DUMP>             find additive sprites in the game's prototype data";

#[derive(Debug, Parser)]
#[command(name = "pastel", override_usage = USAGE, about = "Recolors Factorio's graphics into a brighter, pastel look.")]
struct Cli {
    // The project folder; hidden, for tests and scripts.
    #[arg(long, default_value = ".", hide = true)]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Recolor everything into data/*/updated_graphics
    Build {
        /// Only rebuild these categories (comma-separated, wildcards ok, e.g.
        /// 'terrain.*,ore.*,shadow,lut'); everything else stays as it is
        #[arg(long)]
        only: Option<String>,
        /// Delete existing output and rebuild from scratch
        #[arg(long)]
        clean: bool,
        /// Skip the already-recolored-input check
        #[arg(long)]
        force: bool,
        /// Parallel worker threads (default: CPU count - 1)
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// Check the output against the source
    Validate {
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// Write build/classification_report.csv
    Classify,
    /// Write build/inventory.csv
    Inventory,
    /// Assemble the Factorio mod (Lua sources + recolored graphics) in build/mod/
    Mod {
        /// The mod's source folder, with info.json
        #[arg(default_value = "mods/pastel-factorio")]
        source: PathBuf,
    },
    /// Find the sprites the game draws additively or as light (in the output
    /// of `factorio --dump-data`) and update their rules in pastel.toml
    Scan {
        /// Path to data-raw-dump.json
        dump: PathBuf,
    },
    /// Before/after contact sheets per category (after a build)
    Sheets {
        /// Category name or wildcard, e.g. 'terrain.*'
        #[arg(long)]
        category: Option<String>,
        /// Max files per sheet
        #[arg(long, default_value_t = 48)]
        limit: usize,
    },
}

/// Recolor one PNG in memory and show before and after.
#[derive(Debug, Parser)]
#[command(name = "pastel --preview")]
struct PreviewCli {
    /// Filename or part of a path, e.g. grass-1.png, iron-ore, entity/lab/lab.png
    #[arg(long, value_name = "NAME")]
    preview: String,
    /// Choose match N without prompting
    #[arg(long)]
    pick: Option<usize>,
    /// Scale factor (default: fit about 1600px wide)
    #[arg(long)]
    zoom: Option<f64>,
    /// x,y,w,h region of the source image, e.g. 0,320,256,256
    #[arg(long)]
    crop: Option<String>,
    /// Background behind transparent pixels (default checker; grass for shadows)
    #[arg(long, value_enum)]
    bg: Option<Bg>,
    /// Repeat the (cropped) image N x N, for terrain
    #[arg(long)]
    tile: Option<u32>,
    /// Re-render whenever config files change
    #[arg(long)]
    watch: bool,
    /// Save only, don't open a viewer
    #[arg(long)]
    no_open: bool,
    #[arg(long, default_value = ".", hide = true)]
    root: PathBuf,
}

// The values of --bg. No doc comments on the variants: clap would show them
// in --help.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Bg {
    Checker,
    Grass,
    Light,
    Dark,
}

impl From<Bg> for Background {
    fn from(bg: Bg) -> Background {
        match bg {
            Bg::Checker => Background::Checker,
            Bg::Grass => Background::Grass,
            Bg::Light => Background::Light,
            Bg::Dark => Background::Dark,
        }
    }
}

/// A parsed command line, ready to run.
#[derive(Debug)]
pub struct Invocation(Parsed);

#[derive(Debug)]
enum Parsed {
    Preview(PreviewCli),
    Command(Cli),
}

impl Invocation {
    /// Parse this process's command line. On bad arguments, or for --help,
    /// prints clap's message and exits.
    pub fn from_env() -> Invocation {
        Self::parse_from(std::env::args())
    }

    /// Parse a command line, program name first. `--preview` anywhere in it
    /// selects the preview options instead of the subcommands.
    pub fn parse_from<I: IntoIterator<Item = String>>(args: I) -> Invocation {
        let argv: Vec<String> = args.into_iter().collect();
        if argv
            .iter()
            .skip(1)
            .any(|a| a == "--preview" || a.starts_with("--preview="))
        {
            return Invocation(Parsed::Preview(PreviewCli::parse_from(&argv)));
        }
        Invocation(Parsed::Command(Cli::parse_from(&argv)))
    }

    /// Run the command. Without one, prints the usage summary.
    pub fn run(self) -> Result<()> {
        let cli = match self.0 {
            Parsed::Preview(a) => {
                let args = PreviewArgs {
                    name: a.preview,
                    pick: a.pick,
                    zoom: a.zoom,
                    crop: a.crop,
                    bg: a.bg.map(Background::from),
                    tile: a.tile,
                    watch: a.watch,
                    no_open: a.no_open,
                };
                return preview::run(Project::open(&a.root)?, &args);
            }
            Parsed::Command(cli) => cli,
        };
        let Some(cmd) = cli.cmd else {
            println!("{USAGE}");
            return Ok(());
        };
        let p = Project::open(&cli.root)?;
        match cmd {
            Cmd::Build {
                only,
                clean,
                force,
                jobs,
            } => {
                let args = build::BuildArgs {
                    only,
                    clean,
                    force,
                    jobs,
                };
                build::run(&p, &args)?;
            }
            Cmd::Validate { jobs } => {
                validate::run(&p, jobs)?;
            }
            Cmd::Classify => report::classify(&p)?,
            Cmd::Inventory => report::inventory(&p)?,
            Cmd::Mod { source } => package::run(&p, &p.root.join(source))?,
            Cmd::Scan { dump } => scan::run(&p, &dump)?,
            Cmd::Sheets { category, limit } => sheets::run(&p, category.as_deref(), limit)?,
        }
        Ok(())
    }
}
