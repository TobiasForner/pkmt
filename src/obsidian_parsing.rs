use std::collections::HashMap;

use anyhow::{bail, Result};

use crate::document_component::{collapse_text, DocumentComponent, DocumentElement};
use logos::{Lexer, Logos};

#[derive(Logos, Debug, PartialEq)]
enum ObsidianToken {
    // Can be the start of a heading or part of a reference (e.g. [[file.md#Heading]])
    #[token("#")]
    SingleHash,
    #[token("```ad-note")]
    AdNoteStart,

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
    #[token("\n")]
    Newline,
    #[token("|")]
    Pipe,
    // Or regular expressions.
    #[regex("[-a-zA-Z_]+")]
    Name,
    #[regex("[.{}^$><,0-9():=*&/;'+!?]+")]
    MiscText,
    #[token("\\")]
    Backslash,
}

pub fn parse_obsidian(text: &str) -> Result<Vec<DocumentComponent>> {
    use ObsidianToken::*;

    let mut lexer = ObsidianToken::lexer(&text);
    let mut res = vec![];

    while let Some(result) = lexer.next() {
        //println!("current: {res[]:?}");
        match result {
            Ok(token) => match token {
                EmbedStart => {
                    let parsed = parse_file_link(&mut lexer);
                    // no rename for file embeds
                    if let Ok((name, section, _)) = parsed {
                        res.push(DocumentComponent::new(DocumentElement::FileEmbed(
                            name, section,
                        )));
                    } else {
                        panic!("Something went wrong when trying to parse file embed: {parsed:?}")
                    }
                }
                SingleHash => {
                    let elem = parse_heading(&mut lexer)?;
                    let comp = DocumentComponent::new(elem);
                    res.push(comp);
                }
                Name => res.push(DocumentComponent::new(DocumentElement::Text(
                    lexer.slice().to_string(),
                ))),
                AdNoteStart => {
                    res.push(DocumentComponent::new(parse_adnote(&mut lexer)?));
                }
                Space => {
                    res.push(DocumentComponent::new(DocumentElement::Text(
                        lexer.slice().to_string(),
                    )));
                }
                Newline => {
                    res.push(DocumentComponent::new(DocumentElement::Text(
                        "\n".to_string(),
                    )));
                }
                Pipe => {
                    res.push(DocumentComponent::new_text("|"));
                }
                Backslash => {
                    res.push(DocumentComponent::new_text("\\"));
                }
                OpenDoubleBraces => {
                    let parsed = parse_file_link(&mut lexer);
                    if let Ok((name, section, rename)) = parsed {
                        res.push(DocumentComponent::new(DocumentElement::FileLink(
                            name, section, rename,
                        )));
                    } else {
                        panic!("Something went wrong when trying to parse file link: {parsed:?}")
                    }
                }
                MiscText => res.push(DocumentComponent::new_text(lexer.slice())),
                _ => todo!("Support missing token types: {token:?}"),
            },
            Err(e) => panic!("Error: {e:?};"),
        }
    }
    let res = collapse_text(&res);
    Ok(res)
}

fn parse_heading(lexer: &mut Lexer<'_, ObsidianToken>) -> Result<DocumentElement> {
    let mut level = 1;
    let mut text = String::new();
    while let Some(Ok(token)) = lexer.next() {
        match token {
            ObsidianToken::SingleHash => {
                level += 1;
            }
            ObsidianToken::Space => text.push_str(lexer.slice()),
            ObsidianToken::Name => text.push_str(lexer.slice()),
            ObsidianToken::MiscText => text.push_str(lexer.slice()),
            ObsidianToken::Newline => return Ok(DocumentElement::Heading(level, text)),
            _ => bail!("Failed to parse heading! Encountered {token:?}"),
        }
    }
    bail!("Failed to parse heading!")
}

fn parse_adnote(lexer: &mut Lexer<'_, ObsidianToken>) -> Result<DocumentElement> {
    let mut text = String::new();
    while let Some(Ok(token)) = lexer.next() {
        match token {
            ObsidianToken::TripleBackQuote => {
                let text = text.trim_start_matches("\n").trim_end_matches("\n");
                let mut properties = HashMap::new();
                let mut body_lines = vec![];
                // parse additional properties
                for line in text.lines() {
                    if line.starts_with("title: ") {
                        let remainder = line.strip_prefix("title: ").unwrap();
                        properties.insert("title".to_string(), remainder.trim().to_string());
                    } else {
                        body_lines.push(line);
                    }
                }
                let text = body_lines.join("\n");
                return Ok(DocumentElement::Admonition(text, properties));
            }
            other => {
                let txt = lexer.slice();
                println!("adnote: {other:?} ({txt})");
                text.push_str(txt)
            }
        }
    }
    bail!("Failed to parse adnote!")
}

fn parse_file_link(
    lexer: &mut Lexer<'_, ObsidianToken>,
) -> Result<(String, Option<String>, Option<String>)> {
    use ObsidianToken::*;
    let Some(Ok(Name)) = lexer.next() else {
        panic!("")
    };
    let mut name = lexer.slice().to_string();
    let mut section = None;
    let mut rename = None;
    let mut awaiting_section = false;
    let mut awaiting_rename = false;

    let extend_opt = {
        |s: &Option<String>, ext: &str| {
            let mut res = s.clone().unwrap_or(String::new());
            res.push_str(ext);
            Some(res)
        }
    };
    while let Some(Ok(token)) = lexer.next() {
        match token {
            ClosingDoubleBraces => return Ok((name, section, rename)),
            SingleHash => {
                awaiting_section = true;
            }
            Pipe => {
                awaiting_rename = true;
                awaiting_section = false;
            }
            Name => {
                if awaiting_section {
                    section = extend_opt(&section, lexer.slice());
                } else if awaiting_rename {
                    rename = extend_opt(&rename, lexer.slice());
                }
            }
            MiscText => name.push_str(lexer.slice()),
            Space => name.push_str(lexer.slice()),
            _ => bail!("Encountered {token:?} during parse_file_link!"),
        }
    }
    bail!("Failed to parse file link!")
}

#[test]
fn test_admonition() {
    let text = "```ad-note
title: Title
Some text with $x+1$ math...
A new line!
```";

    let res = parse_obsidian(text);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = vec![DocumentComponent::new(
            crate::obsidian_parsing::DocumentElement::Admonition(
                "Some text with $x+1$ math...\nA new line!".to_string(),
                props,
            ),
        )];
        assert_eq!(res, expected);
    } else {
        assert!(false, "Got {res:?}")
    }
}
