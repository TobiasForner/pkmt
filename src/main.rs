use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use todoi::{get_zk_creator_file, set_zk_creator_file};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
extern crate tracing;

mod file_checklist;
use document_component::{convert_file, convert_tree, FileInfo};
use file_checklist::checklist_for_tree;
use inspect::{list_empty_files, similar_file_names};
use parse::TextMode;
use util::files_in_tree;

use std::{collections::HashSet, fmt::Debug, path::PathBuf};

use crate::todoi::config::Tags;
mod document_component;
mod inspect;
mod logseq_parsing;

mod obsidian_parsing;
mod parse;
mod todoi;
mod util;
mod zk_parsing;

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
        #[arg(value_enum)]
        inmode: TextMode,

        /// parsing mode
        #[arg(value_enum)]
        outmode: TextMode,

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
    Inspect {
        /// root directory to inspect
        #[arg(required = true)]
        root_dir: PathBuf,
    },
    Test {},
    Todoi {
        #[arg(required = false)]
        graph_root: Option<PathBuf>,
        #[arg(short, long, default_value_t = false, required = false)]
        complete_tasks: bool,
        #[arg(short, long, required = false)]
        mode: Option<TextMode>,
    },
    TodoiConfig {
        #[clap(subcommand)]
        tcfg_command: TCfgCommand,
    },
    Creator {
        #[arg(required = true)]
        root_dir: PathBuf,
        #[arg(required = true)]
        name: String,
        #[arg(short, long, required = false)]
        mode: Option<TextMode>,
        #[clap(subcommand)]
        creator_command: CreatorCommand,
    },
}

#[derive(Clone, Subcommand)]
enum TCfgCommand {
    ShowPaths,
    AddYtTags {
        #[arg(required = true)]
        channel: String,
        #[clap(required = true)]
        tags: Vec<String>,
    },
    AddKwTags {
        #[arg(required = true)]
        kw: String,
        #[clap(required = true)]
        tags: Vec<String>,
    },
    AddUrlTags {
        #[arg(required = true)]
        url: String,
        #[clap(required = true)]
        tags: Vec<String>,
    },
    AddUrlSources {
        #[arg(required = true)]
        url: String,
        #[clap(required = true)]
        sources: Vec<String>,
    },
}

#[derive(Clone, Subcommand)]
enum CreatorCommand {
    Delete,
    Overwrite {
        #[arg(required = true)]
        new_file: PathBuf,
    },
    ShowFile {
        #[arg(short, long)]
        relative: Option<PathBuf>,
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

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let res: Result<()> = match cli.command {
        Some(Commands::Todoi {
            graph_root,
            complete_tasks,
            mode,
        }) => {
            let mode = mode.unwrap_or(TextMode::LogSeq);
            let graph_root = if let Some(graph_root) = graph_root {
                graph_root
            } else if mode == TextMode::Zk {
                if let Ok(notebook_dir) = std::env::var("ZK_NOTEBOOK_DIR") {
                    PathBuf::from(notebook_dir)
                } else {
                    bail!("Could not determine zk notebook dir. Either specify it via the environment variable 'ZK_NOTEBOOK_DIR' or specify it directly!");
                }
            } else {
                bail!("Could not determine graph root!");
            };
            todoi::main(graph_root, complete_tasks, mode)?;
            Ok(())
        }
        Some(Commands::TodoiConfig { tcfg_command }) => match tcfg_command {
            TCfgCommand::ShowPaths => {
                crate::todoi::config::Config::show_paths();
                Ok(())
            }
            TCfgCommand::AddYtTags { channel, tags } => {
                let mut all_tags = Tags::parse()?;
                all_tags.add_yt_tags(channel, tags)
            }
            TCfgCommand::AddKwTags { kw, tags } => {
                let mut all_tags = Tags::parse()?;
                all_tags.add_kw_tags(kw, tags)
            }
            TCfgCommand::AddUrlTags { url, tags } => {
                let mut all_tags = Tags::parse()?;
                all_tags.add_url_tags(url, tags)
            }
            TCfgCommand::AddUrlSources { url, sources } => {
                let mut all_tags = Tags::parse()?;
                all_tags.add_url_sources(url, sources)
            }
        },
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
        Some(Commands::Inspect { root_dir }) => {
            list_empty_files(root_dir.clone())?;
            similar_file_names(root_dir, 4);
            Ok(())
        }
        Some(Commands::Test {}) => {
            println!("Only prints this message atm");
            Ok(())
        }
        Some(Commands::Convert {
            in_path,
            out_path,
            inmode,
            outmode,
            imdir,
            imout,
        }) => {
            let mut imdir = imdir;
            let mut imout = imout;
            if let (Some(im_in), Some(im_out)) = (&imdir, &imout) {
                if !im_out.exists() {
                    std::fs::create_dir_all(im_out)?;
                }
                imdir = Some(im_in.canonicalize()?);
                imout = Some(im_out.canonicalize()?);
            }
            let mentioned_files = if in_path.is_dir() {
                convert_tree(in_path, out_path, inmode, outmode, &imdir, &imout)
            } else {
                let file_info =
                    FileInfo::try_new(in_path, Some(out_path), imdir.clone(), imout.clone())?;
                convert_file(file_info, inmode, outmode)
            }?;

            let mentioned_files: HashSet<String> = HashSet::from_iter(mentioned_files);

            if let (Some(imdir), Some(imout)) = (imdir, imout) {
                let found_image_files = files_in_tree(&imdir, &Some(vec!["png"]))?;
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

                let _: () = matched_files.into_iter().try_for_each(|f| {
                    let rel = pathdiff::diff_paths(&f, &imdir)
                        .context(format!("Could not get relative path for {f:?}"))?;
                    let target = imout.join(&rel);
                    std::fs::copy(f, target)?;
                    Ok::<(), anyhow::Error>(())
                })?;
            }
            Ok(())
        }
        Some(Commands::Creator {
            root_dir,
            name,
            mode,
            creator_command,
        }) => {
            let mode = mode.unwrap_or(TextMode::Zk);
            match mode {
                TextMode::Zk => {
                    match creator_command {
                        CreatorCommand::Delete => {
                            todo!("not implemented!")
                        }
                        CreatorCommand::Overwrite { new_file } => {
                            set_zk_creator_file(&name, &new_file)?;
                        }
                        CreatorCommand::ShowFile { relative } => {
                            let mut file = get_zk_creator_file(&root_dir, &name)?;
                            if let Some(relative) = relative {
                                if let Some(rel) = relative.parent() {
                                    if let Some(rel) = pathdiff::diff_paths(&file, rel) {
                                        file = rel.to_path_buf();
                                    }
                                }
                            }
                            println!("{}", file.to_string_lossy());
                        }
                    }
                    Ok(())
                }
                _ => todo!("to implement: retrieve creator file for {mode:?}"),
            }
        }
        None => panic!("Failed to parse arguments!"),
    };
    res
}
