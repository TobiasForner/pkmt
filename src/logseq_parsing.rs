use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use logos::{Lexer, Logos};

use crate::{
    document_component::{DocumentComponent, DocumentElement, ParsedDocument},
    util::indent_level,
};

#[derive(Logos, Debug, PartialEq)]
enum LogSeqToken {
    // Can be the start of a heading or part of a reference (e.g. [[file.md#Heading]])
    #[token("#")]
    SingleHash,
    #[token("```")]
    TripleBackQuote,

    #[token("![[")]
    EmbedStart,

    #[token("[[")]
    OpenDoubleBraces,
    #[token("]]")]
    ClosingDoubleBraces,
    #[regex("[ \t]+")]
    Space,
    #[regex("\n\r?")]
    Newline,
    #[regex("\r")]
    CarriageReturn,
    #[token("|")]
    Pipe,
    #[token("[")]
    Bracket,
    #[token("]")]
    ClosingBracket,
    // Or regular expressions.
    #[regex("[a-zA-Z_]+")]
    Name,
    #[token("-")]
    Minus,
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"]+")]
    MiscText,
    #[token("\\")]
    Backslash,
}

pub fn parse_logseq_file<T: AsRef<Path>>(file_path: T) -> Result<ParsedDocument> {
    let file_path = file_path.as_ref().canonicalize()?;
    let text = std::fs::read_to_string(&file_path)?;

    let file_dir = file_path
        .parent()
        .context(format!("{file_path:?} has no parent!"))?
        .to_path_buf();

    let pt = parse_logseq_text(&text, &Some(file_dir))?;
    Ok(ParsedDocument::ParsedFile(pt.into_components(), file_path))
}

pub fn parse_logseq_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use LogSeqToken::*;
    //let text = apply_substitutions(text);

    let mut lexer = LogSeqToken::lexer(&text);
    let mut res = vec![];
    let mut new_line_or_whitespace = true;

    let mut text = String::new();
    while let Some(result) = lexer.next() {
        match result {
            Ok(token) => match token {
                Name => res.push(DocumentComponent::new(DocumentElement::Text(
                    lexer.slice().to_string(),
                ))),
                Space => {
                    text.push_str(lexer.slice());
                }
                Newline => {
                    text.push_str(lexer.slice());
                    res.push(DocumentComponent::new_text(&text));
                    text = String::new();
                    new_line_or_whitespace = true;
                }
                Minus => {
                    if new_line_or_whitespace {
                        let line = text.lines().last().context("No last line!")?;
                        let indent = indent_level(line);
                        let (de, rem) = parse_list_element(&mut lexer, indent, file_dir)?;
                        res.push(DocumentComponent::new(de));
                        let rec = parse_logseq_text(&rem, file_dir)?;
                        res.extend(rec.into_components());
                    }
                    text.push_str(lexer.slice());
                }
                _ => {
                    new_line_or_whitespace = false;
                    text.push_str(lexer.slice())
                }
            },
            Err(_) => {
                bail!("Error {}", construct_error_details(&lexer))
            }
        }
    }
    //let res = collapse_text(&res);
    Ok(ParsedDocument::ParsedText(res))
}

fn parse_list_element(
    lexer: &mut Lexer<'_, LogSeqToken>,
    indent: usize,
    file_dir: &Option<PathBuf>,
) -> Result<(DocumentElement, String)> {
    use LogSeqToken::*;
    // strategy: parse text as long as the indent level is maintained
    // if indent level is not maintained: we have already consumed a part of the input. We simply return that as well so that the main function can handle it
    let mut text = String::new();
    let mut remainder = String::new();
    while let Some(result) = lexer.next() {
        match result {
            Ok(token) => match token {
                Newline => {
                    if indent_level(&remainder) == indent {
                        text.push_str(&remainder);
                        text.push_str(lexer.slice());
                        remainder = String::new();
                    } else {
                        remainder.push('\n');
                        break;
                    }
                }
                _ => text.push_str(lexer.slice()),
            },
            Err(_) => {
                bail!("Error {}", construct_error_details(&lexer))
            }
        }
    }
    Ok((
        DocumentElement::ListElement(parse_logseq_text(&text, file_dir)?, indent),
        remainder,
    ))
}

fn construct_error_details(lexer: &Lexer<'_, LogSeqToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}
