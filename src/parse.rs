use anyhow::Result;
use clap::{builder::PossibleValue, ValueEnum};
use std::path::PathBuf;

use crate::{
    document_component::ParsedDocument,
    logseq_parsing::{parse_logseq_file, parse_logseq_text},
    obsidian_parsing::{parse_obsidian_file, parse_obsidian_text},
    zk_parsing::{parse_zk_file, parse_zk_text},
};

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum TextMode {
    Obsidian,
    LogSeq,
    Zk,
}

impl ValueEnum for TextMode {
    fn value_variants<'a>() -> &'a [Self] {
        use TextMode::*;
        &[Obsidian, LogSeq, Zk]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        use TextMode::*;
        Some(match self {
            Obsidian => PossibleValue::new("obsidian"),
            LogSeq => PossibleValue::new("logseq"),
            Zk => PossibleValue::new("zk"),
        })
    }
}
pub fn parse_text(
    text: &str,
    mode: &TextMode,
    file_dir: &Option<PathBuf>,
) -> Result<ParsedDocument> {
    use TextMode::*;
    match mode {
        Obsidian => parse_obsidian_text(text, file_dir),
        LogSeq => parse_logseq_text(text, file_dir),
        Zk => parse_zk_text(text, file_dir),
    }
}

pub fn parse_file(file: &PathBuf, mode: TextMode) -> Result<ParsedDocument> {
    use TextMode::*;
    match mode {
        Obsidian => parse_obsidian_file(file),
        LogSeq => parse_logseq_file(file),
        Zk => parse_zk_file(file),
    }
}
