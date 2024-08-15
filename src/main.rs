use clap::Parser;
use obsidian_parsing::parse_obsidian;

use std::path::PathBuf;
mod document_component;

mod obsidian_parsing;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// file to parse
    #[arg(short, long)]
    file: PathBuf,

    ///parsing mode
    #[arg(short, long)]
    mode: Option<String>,
}

fn main() {
    let args = Args::parse();
    let file = args.file;
    let text = std::fs::read_to_string(&file).unwrap();
    let text = apply_substitutions(&text);
    let mode = args.mode.unwrap_or(String::from("Obsidian"));
    let res = match mode.as_str() {
        "Obsidian" => parse_obsidian(&text),
        _ => panic!("Unsupported mode: {mode}"),
    };
    println!("{res:?}");
}

fn apply_substitutions(text: &str) -> String {
    text.replace('−', "-")
        .replace('∗', "*")
        .replace('∈', "\\in ")
}
