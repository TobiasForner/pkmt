use anyhow::{bail, Context, Result};
use clap::Parser;
use obsidian_parsing::parse_obsidian;

use std::path::{Path, PathBuf};
mod document_component;

mod obsidian_parsing;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// file to parse
    #[arg(short, long)]
    file: PathBuf,

    /// destination file to write to
    #[arg(short, long)]
    out_file: PathBuf,

    ///parsing mode
    #[arg(short, long)]
    mode: Option<String>,
}

fn main() {
    let args = Args::parse();
    let file = args.file;
    let out_file = args.out_file;
    let mode = args.mode.unwrap_or("Obsidian".to_string());
    let res = convert_tree(file, out_file, &mode);
    println!("{res:?}");
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
            let text = std::fs::read_to_string(&f)?;
            let text = apply_substitutions(&text);
            let res = match mode {
                "Obsidian" => parse_obsidian(&text),
                _ => panic!("Unsupported mode: {mode}"),
            };
            //println!("{res:?}");
            if let Ok(components) = res {
                let lines: Vec<String> = components.iter().map(|c| c.to_logseq_text()).collect();
                let text = lines.join("\n");

                let rel = pathdiff::diff_paths(&f, &root_dir).unwrap();
                let target = target_dir.join(&rel);
                println!("{f:?} --> {target:?} ({rel:?})");
                let res =
                    std::fs::write(&target, text).context(format!("Failed to write to {target:?}"));
                if res.is_err() {
                    bail!("Encountered: {res:?}!");
                }
                Ok(())
            } else {
                bail!("Could not convert the file {f:?} to obsidian: {res:?}")
            }
        })
        .collect::<Result<()>>()?;
    Ok(())
}

fn files_in_tree<T: AsRef<Path>>(root_dir: T) -> Result<Vec<PathBuf>> {
    let mut res = vec![];
    let root_dir = root_dir.as_ref().canonicalize()?;
    let dir_entry = root_dir.read_dir()?;
    let tmp: Result<()> = dir_entry
        .into_iter()
        .map(|f| {
            let path = f.unwrap().path();
            if let Some(ext) = path.extension() {
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
