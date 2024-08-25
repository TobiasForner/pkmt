use anyhow::Result;
use clap::{builder::PossibleValue, ValueEnum};
use std::path::PathBuf;

use crate::{
    document_component::ParsedDocument, logseq_parsing::parse_logseq_file,
    obsidian_parsing::parse_obsidian_file,
};

#[derive(Clone, Debug)]
pub enum ParseMode {
    Obsidian,
    LogSeq,
}

impl ValueEnum for ParseMode {
    fn value_variants<'a>() -> &'a [Self] {
        use ParseMode::*;
        &[Obsidian, LogSeq]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        use ParseMode::*;
        Some(match self {
            Obsidian => PossibleValue::new("obsidian"),
            LogSeq => PossibleValue::new("logseq"),
        })
    }
}

pub fn parse_file(file: &PathBuf, mode: ParseMode) -> Result<ParsedDocument> {
    match mode {
        ParseMode::Obsidian => parse_obsidian_file(file),
        ParseMode::LogSeq => parse_logseq_file(file),
    }
}
