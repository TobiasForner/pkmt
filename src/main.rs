use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use document_component::to_logseq_text;
use obsidian_parsing::parse_obsidian;
mod file_checklist;
use file_checklist::checklist_for_tree;

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};
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
        #[arg(short, long)]
        in_path: PathBuf,

        /// destination path to write to
        #[arg(short, long)]
        out_path: PathBuf,

        /// parsing mode
        #[arg(short, long)]
        mode: Option<String>,
    },
    /// generate a file checklist
    Checklist {
        /// root directory to generate the checklist for
        #[arg(short, long)]
        root_dir: PathBuf,

        /// file to write the checklist to
        #[arg(short, long)]
        out_file: PathBuf,
        /// String to use to signal a todo
        #[arg(short, long)]
        todo_marker: String,
    },
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {}

fn main() {
    let res = run();
    println!("{res:?}");
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

fn apply_substitutions(text: &str) -> String {
    text.replace('−', "-")
        .replace('∗', "*")
        .replace('∈', "\\in ")
        .replace("“", "\"")
        .replace("”", "\"")
        .replace("∃", "EXISTS")
        .replace("’", "'")
        .replace("_", "_")
        .replace("–", "-")
}

fn convert_tree(root_dir: PathBuf, target_dir: PathBuf, mode: &str) -> Result<()> {
    let root_dir = root_dir.canonicalize()?;
    let files = files_in_tree(&root_dir)?;
    let _ = std::fs::create_dir_all(&target_dir)?;
    let target_dir = target_dir.canonicalize()?;
    println!("target: {target_dir:?}");

    let _ = files
        .iter()
        .map(|f| {
            let rel = pathdiff::diff_paths(&f, &root_dir).unwrap();
            let target = target_dir.join(&rel);
            convert_file(f, &target, &mode)
        })
        .collect::<Result<()>>()?;
    Ok(())
}

fn convert_file<T: AsRef<Path> + Debug, U: AsRef<Path> + Debug>(
    file: T,
    out_file: U,
    mode: &str,
) -> Result<()> {
    let file = file.as_ref();
    let text = std::fs::read_to_string(&file)?;
    let text = apply_substitutions(&text);
    let res = match mode {
        "Obsidian" => parse_obsidian(&text),
        _ => panic!("Unsupported mode: {mode}"),
    };

    if let Ok(components) = res {
        let text = to_logseq_text(&components);

        let res =
            std::fs::write(&out_file, text).context(format!("Failed to write to {out_file:?}"));
        if res.is_err() {
            bail!("Encountered: {res:?}!");
        }
        Ok(())
    } else {
        bail!("Could not convert the file {file:?} to obsidian: {res:?}")
    }
}

fn files_in_tree<T: AsRef<Path>>(root_dir: T) -> Result<Vec<PathBuf>> {
    let mut res = vec![];
    let root_dir = root_dir.as_ref().canonicalize()?;
    let dir_entry = root_dir.read_dir()?;
    let tmp: Result<()> = dir_entry
        .into_iter()
        .map(|f| {
            let path = f.unwrap().path();
            if path.is_dir() {
                let rec = files_in_tree(&path)?;
                res.extend(rec.into_iter());
            } else if let Some(ext) = path.extension() {
                if ["md"].contains(&ext.to_str().unwrap_or("should not be found")) {
                    res.push(path.clone());
                }
            }
            Ok(())
        })
        .collect();
    if tmp.is_err() {
        bail!("Encountered error: {tmp:?}!")
    }
    Ok(res)
}
