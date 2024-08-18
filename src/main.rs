use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
mod file_checklist;
use document_component::{convert_file, convert_tree};
use file_checklist::checklist_for_tree;

use std::{fmt::Debug, path::PathBuf};
mod document_component;

mod obsidian_parsing;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// convert between different formats
    Convert {
        /// path to parse. If this is a directory, the out_path must also be a directory. Missing directories in out_path will be created.
        #[arg(required = true)]
        in_path: PathBuf,

        /// destination path to write to
        #[arg(required = true)]
        out_path: PathBuf,

        /// parsing mode
        #[arg(short, long)]
        mode: Option<String>,
    },
    /// generate a file checklist
    Checklist {
        /// root directory to generate the checklist for
        #[arg(required = true)]
        root_dir: PathBuf,

        /// file to write the checklist to
        #[arg(required = true)]
        out_file: PathBuf,
        /// String to use to signal a todo
        #[arg(required = true)]
        todo_marker: String,
    },
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {}

fn main() {
    let res = run();
    if res.is_err() {
        println!("{res:?}");
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let res: Result<()> = match cli.command {
        Some(Commands::Checklist {
            root_dir,
            out_file,
            todo_marker,
        }) => {
            let res = checklist_for_tree(root_dir, &todo_marker)?;
            let _ = std::fs::write(&out_file, res)
                .context(format!("Could not write checklist to {out_file:?}!"))?;
            Ok(())
        }
        Some(Commands::Convert {
            in_path,
            out_path,
            mode,
        }) => {
            let mode = mode.unwrap_or("Obsidian".to_string());
            if in_path.is_dir() {
                let _ = convert_tree(in_path, out_path, &mode)?;
            } else {
                let _ = convert_file(in_path, out_path, &mode)?;
            }
            Ok(())
        }
        None => panic!("Failed to parse arguments!"),
    };
    res
}
