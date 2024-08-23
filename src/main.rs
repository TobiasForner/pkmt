use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
mod file_checklist;
use document_component::{convert_file, convert_tree, files_in_tree};
use file_checklist::checklist_for_tree;

use std::{collections::HashSet, fmt::Debug, path::PathBuf};
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

        /// image directory for the input files. If this is set, found image files will be copied to the output image dir `imout` (required in this case)
        #[arg(long)]
        imdir: Option<PathBuf>,

        #[arg(long)]
        imout: Option<PathBuf>,
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
            std::fs::write(&out_file, res)
                .context(format!("Could not write checklist to {out_file:?}!"))?;
            Ok(())
        }
        Some(Commands::Convert {
            in_path,
            out_path,
            mode,
            imdir,
            imout,
        }) => {
            if imdir.is_none() && imout.is_some() || imdir.is_some() && imout.is_none() {
                bail!("Either both imdir and imout need to be specified or none of them!")
            }
            let mode = mode.unwrap_or("Obsidian".to_string());
            let mentioned_files = if in_path.is_dir() {
                convert_tree(in_path, out_path, &mode, &imout)
            } else {
                convert_file(in_path, out_path, &mode, &imdir)
            }?;

            let mentioned_files: HashSet<String> = HashSet::from_iter(mentioned_files);

            if let Some(imdir) = imdir {
                if let Some(imout) = imout {
                    let found_image_files = files_in_tree(&imdir, &Some(vec!["png"]))?;
                    //println!("{found_image_files:?}");
                    let matched_files: Vec<PathBuf> = found_image_files
                        .into_iter()
                        .filter(|f| {
                            let Some(file_name) = f.file_name() else {
                                return false;
                            };
                            let Some(file_name) = file_name.to_str() else {
                                return false;
                            };
                            if mentioned_files.contains(file_name) {
                                return true;
                            }
                            let file_name = PathBuf::from(file_name);
                            let Some(file_name) = file_name.file_stem() else {
                                return false;
                            };
                            let Some(file_name) = file_name.to_str() else {
                                return false;
                            };
                            if mentioned_files.contains(file_name) {
                                return true;
                            }
                            false
                        })
                        .collect();

                    let imdir = imdir.canonicalize()?;
                    let imout = imout.canonicalize()?;
                    if !imout.exists() {
                        std::fs::create_dir(&imout)?;
                    }
                    let _: () = matched_files.into_iter().try_for_each(|f| {
                        let rel = pathdiff::diff_paths(&f, &imdir)
                            .context(format!("Could not get relative path for {:?}", f))?;
                        let target = imout.join(&rel);
                        std::fs::copy(f, target)?;
                        Ok::<(), anyhow::Error>(())
                    })?;
                }
            }
            Ok(())
        }
        None => panic!("Failed to parse arguments!"),
    };
    res
}
