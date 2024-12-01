use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::{
    obsidian_parsing::parse_obsidian_text,
    util::{apply_substitutions, indent_level, trim_like_first_line_plus},
};
use anyhow::{bail, Context, Result};

use crate::document_component::{
    collapse_text, DocumentComponent, DocumentElement, MentionedFile, ParsedDocument,
};
use logos::{Lexer, Logos};

#[derive(Logos, Debug, PartialEq)]
enum ZkToken {
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
    #[regex("[-a-zA-Z_]+")]
    Name,
    #[token("- ")]
    ListStart,
    #[regex(r"[a-zA-Z_]+\s*::=\s*[a-zA-Z_]")]
    SingleProperty,
    #[regex(r"[a-zA-Z_]+\s*::=\s*\[([a-zA-Z_]\s*,\s*)[a-zA-Z_]\]")]
    MultiProperty,
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"]+")]
    MiscText,
    #[token("\\")]
    Backslash,
}

impl ZkToken {
    fn is_blank(&self) -> bool {
        matches!(self, Self::Space)
    }
}

pub fn parse_zk_file<T: AsRef<Path>>(file_path: T) -> Result<ParsedDocument> {
    let file_path = file_path.as_ref().canonicalize()?;
    let text = std::fs::read_to_string(&file_path)?;

    let file_dir = file_path
        .parent()
        .context(format!("{file_path:?} has no parent!"))?
        .to_path_buf();

    let pt = parse_zk_text(&text, &Some(file_dir))?;
    Ok(ParsedDocument::ParsedFile(pt.into_components(), file_path))
}

pub fn parse_zk_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use ZkToken::*;
    let text = apply_substitutions(text);

    let mut lexer = ZkToken::lexer(&text);
    let mut res = vec![];
    let mut blank_line = true;
    let mut indent_spaces = 0;

    while let Some(result) = lexer.next() {
        println!("{result:?}: '{}'", lexer.slice());
        match result {
            Ok(token) => {
                match token {
                    EmbedStart => {
                        let parsed = parse_file_link(&mut lexer, file_dir);
                        // no rename for file embeds
                        if let Ok((name, section, _)) = parsed {
                            res.push(DocumentComponent::new(DocumentElement::FileEmbed(
                                name, section,
                            )));
                        } else {
                            panic!(
                                "Something went wrong when trying to parse file embed: {parsed:?}"
                            )
                        }
                    }
                    SingleProperty => {
                        let parts = lexer
                            .slice()
                            .split_once("::=")
                            .context("Single property must contain '::='!")?;
                        let prop_name = parts.0.trim();
                        let prop_value = parts.0.trim();
                        res.push(DocumentComponent::new(DocumentElement::Properties(vec![(
                            prop_name.to_string(),
                            vec![prop_value.to_string()],
                        )])));
                    }
                    MultiProperty => {
                        let parts = lexer
                            .slice()
                            .split_once("::=")
                            .context("Single property must contain '::='!")?;
                        let prop_name = parts.0.trim();
                        let prop_values = parts.0.trim().replace("[", "").replace("]", "");
                        let prop_values = prop_values
                            .split(",")
                            .map(|s| s.trim().to_string())
                            .collect();
                        res.push(DocumentComponent::new(DocumentElement::Properties(vec![(
                            prop_name.to_string(),
                            prop_values,
                        )])));
                    }
                    SingleHash => {
                        if blank_line {
                            let elem = parse_heading(&mut lexer)?;
                            let comp = DocumentComponent::new(elem);
                            res.push(comp);
                            res.push(DocumentComponent::new_text("\n"));
                        } else {
                            res.push(DocumentComponent::new_text("#"));
                        }
                    }
                    Name => res.push(DocumentComponent::new(DocumentElement::Text(
                        lexer.slice().to_string(),
                    ))),
                    AdNoteStart => {
                        res.push(DocumentComponent::new(parse_adnote(&mut lexer, file_dir)?));
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
                        blank_line = true;
                    }
                    Pipe => {
                        res.push(DocumentComponent::new_text("|"));
                    }
                    Bracket => {
                        res.push(DocumentComponent::new_text("["));
                    }
                    ClosingBracket => {
                        res.push(DocumentComponent::new_text("]"));
                    }
                    Backslash => {
                        res.push(DocumentComponent::new_text("\\"));
                    }
                    OpenDoubleBraces => {
                        let parsed = parse_file_link(&mut lexer, file_dir);
                        if let Ok((name, section, rename)) = parsed {
                            res.push(DocumentComponent::new(DocumentElement::FileLink(
                                name, section, rename,
                            )));
                        } else {
                            bail!("Something went wrong when trying to parse file link: {parsed:?}")
                        }
                    }
                    MiscText => res.push(DocumentComponent::new_text(lexer.slice())),
                    CarriageReturn => {
                        res.push(DocumentComponent::new_text("\r"));
                    }
                    ListStart => {
                        if blank_line {
                            if let Ok(le) = parse_list_element(&mut lexer, indent_spaces, file_dir)
                            {
                                let mut comps = le.into_components();
                                res.append(&mut comps);
                            }
                        } else {
                            res.push(DocumentComponent::new_text("- "));
                        }
                    }
                    _ => todo!("Support missing token types: {token:?}"),
                }

                if !token.is_blank() {
                    blank_line = false;
                } else if blank_line {
                    indent_spaces += lexer.slice().replace("\t", "    ").len();
                }
            }
            Err(_) => {
                bail!("Error {}", construct_error_details(&lexer))
            }
        }
    }
    let res = collapse_text(&res);
    Ok(ParsedDocument::ParsedText(res))
}

fn construct_error_details(lexer: &Lexer<'_, ZkToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}

fn parse_list_element(
    lexer: &mut Lexer<'_, ZkToken>,
    initial_indent_spaces: usize,
    file_dir: &Option<PathBuf>,
) -> Result<ParsedDocument> {
    // determine the extent of the list
    let mut last_was_blank = false;
    let mut blank_line = false;
    let mut text = " ".repeat(initial_indent_spaces);
    while let Some(Ok(token)) = lexer.next() {
        text.push_str(lexer.slice());
        if !token.is_blank() {
            blank_line = false;
        }
        if token == ZkToken::Newline {
            last_was_blank = blank_line;
            blank_line = true;
        }
        if blank_line && last_was_blank {
            break;
        }
    }
    let mut list_components = vec![];
    let mut current = String::new();
    let mut current_indent = 0;

    text.lines().for_each(|l| {
        if l.trim().starts_with("- ") {
            list_components.push((current.clone(), current_indent));
            current_indent = indent_level(l);

            current = l.to_string();
        } else {
            current.push('\n');
            current.push_str(l);
        }
    });
    if !current.is_empty() {
        list_components.push((current.clone(), current_indent));
    }

    // text contains the text that comprises the list
    //parse_list_element_from_text(&text, file_dir)
    let mut pos = 0;
    let mut components = vec![];
    loop {
        let (comp, new_pos) = assemble_blocks_rec(pos, &list_components, file_dir)?;
        pos = new_pos;
        components.push(comp);
        if pos >= list_components.len() {
            break;
        }
    }
    Ok(ParsedDocument::ParsedText(components))
}

fn assemble_blocks_rec(
    pos: usize,
    block_texts_indent: &[(String, usize)],
    file_dir: &Option<PathBuf>,
) -> Result<(DocumentComponent, usize)> {
    let (block_text, i) = &block_texts_indent[pos];
    let block_text = trim_like_first_line_plus(block_text, 2);
    let first_block = parse_list_element_from_text(&block_text, file_dir)?;
    let mut pos = pos + 1;
    let mut children = vec![];
    while pos < block_texts_indent.len() && block_texts_indent[pos].1 > *i {
        let (child, new_pos) = assemble_blocks_rec(pos, block_texts_indent, file_dir)?;
        children.push(child);
        pos = new_pos;
    }

    Ok((
        DocumentComponent::new_with_children(first_block, children),
        pos,
    ))
}

fn parse_list_element_from_text(text: &str, file_dir: &Option<PathBuf>) -> Result<DocumentElement> {
    let text = text.trim().replacen("- ", "", 1);
    let pd = parse_zk_text(&text, file_dir)?;
    Ok(DocumentElement::ListElement(pd, vec![]))
}
fn parse_heading(lexer: &mut Lexer<'_, ZkToken>) -> Result<DocumentElement> {
    use ZkToken::*;
    let mut level = 1;
    let mut text = String::new();
    while let Some(Ok(token)) = lexer.next() {
        match token {
            SingleHash => {
                level += 1;
            }
            Space => text.push_str(lexer.slice()),
            Name => text.push_str(lexer.slice()),
            MiscText => text.push_str(lexer.slice()),
            CarriageReturn => text.push('\r'),
            Newline => return Ok(DocumentElement::Heading(level, text)),
            Backslash => text.push_str(lexer.slice()),
            other => bail!(
                "Failed to parse heading! {other:?}: {}",
                construct_error_details(lexer)
            ),
        }
    }
    Ok(DocumentElement::Heading(level, text))
}

fn parse_adnote(
    lexer: &mut Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<DocumentElement> {
    let mut text = String::new();
    while let Some(Ok(token)) = lexer.next() {
        match token {
            ZkToken::TripleBackQuote => {
                let text = text.trim_start_matches("\n").trim_end_matches("\n");
                let mut properties = HashMap::new();
                let mut body_text = String::new();
                // parse additional properties
                for line in text.lines() {
                    if line.starts_with("title: ") {
                        let remainder = line.strip_prefix("title: ").unwrap();
                        properties.insert("title".to_string(), remainder.trim().to_string());
                    } else if line.starts_with("color: ") {
                        let remainder = line.strip_prefix("color: ").unwrap();
                        properties.insert("color".to_string(), remainder.trim().to_string());
                    } else {
                        if !body_text.is_empty() {
                            body_text.push('\n');
                        }
                        body_text.push_str(line);
                    }
                }
                let pd = parse_zk_text(&body_text, file_dir)?;
                return Ok(DocumentElement::Admonition(
                    pd.into_components(),
                    properties,
                ));
            }
            _ => {
                let txt = lexer.slice();
                text.push_str(txt)
            }
        }
    }
    bail!(
        "Failed to parse adnote: Could not match '{}' at positions {:?}",
        lexer.slice(),
        lexer.span()
    )
}

fn parse_file_link(
    lexer: &mut Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<(MentionedFile, Option<String>, Option<String>)> {
    use ZkToken::*;
    let mut name = String::new();
    let mut section = None;
    let mut rename = None;
    let mut awaiting_section = false;
    let mut awaiting_rename = false;

    let extend_opt = {
        |s: &Option<String>, ext: &str| {
            let mut res = s.clone().unwrap_or_default();
            res.push_str(ext);
            Some(res)
        }
    };

    while let Some(Ok(token)) = lexer.next() {
        match token {
            ClosingDoubleBraces => {
                let name = name.trim().to_string();
                let mut mf = MentionedFile::FileName(name.clone());
                if let Some(dir) = file_dir {
                    let file = dir.join(&name);
                    if file.exists() {
                        let file = file.canonicalize()?;
                        mf = MentionedFile::FilePath(file);
                    }
                    let Ok(file) = PathBuf::from_str(&name);

                    if file.exists() {
                        mf = MentionedFile::FilePath(file.canonicalize()?);
                    }
                }
                return Ok((mf, section, rename));
            }
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
                } else {
                    name.push_str(lexer.slice());
                }
            }
            MiscText => {
                if awaiting_section {
                    section = extend_opt(&section, lexer.slice());
                } else if awaiting_rename {
                    rename = extend_opt(&rename, lexer.slice());
                } else {
                    name.push_str(lexer.slice());
                }
            }
            Space => {
                if awaiting_section {
                    section = extend_opt(&section, lexer.slice());
                } else if awaiting_rename {
                    rename = extend_opt(&rename, lexer.slice());
                } else {
                    name.push_str(lexer.slice());
                }
            }
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

    let res = parse_zk_text(text, &None);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = ParsedDocument::ParsedText(vec![DocumentComponent::new(
            crate::document_component::DocumentElement::Admonition(
                vec![DocumentComponent::new_text(
                    "Some text with $x+1$ math...\nA new line!",
                )],
                props,
            ),
        )]);
        assert_eq!(res, expected);
    } else {
        panic!("Got {res:?}")
    }
}

#[test]
fn test_text_parsing() {
    use DocumentElement::*;
    let text = "Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the [[MaximumClique|maximum clique problem]], unless P = NP.";
    let res = parse_zk_text(text, &None);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = ParsedDocument::ParsedText(vec![DocumentComponent::new(Text("Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the ".to_string())), DocumentComponent::new(FileLink(MentionedFile::FileName("MaximumClique".to_string()), None, Some("maximum clique problem".to_string()))), DocumentComponent::new(Text(", unless P = NP.".to_string()))]);
        assert_eq!(res, expected);
    } else {
        panic!("Got {res:?}")
    }
}

#[test]
fn test_image_embed_conversion() {
    let test_text =
        "variables will become nonzero, so we only need to keep track of these nonzero variables.

#### Initial Algorithm";
    let res = parse_zk_text(test_text, &None);
    if let Ok(pd) = res {
        println!("{pd:?}");
        let zk_text = pd.to_zk_text(&None);
        assert_eq!(zk_text, test_text);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_simple_list() {
    let text = "- item 1\n- item 2";
    let res = parse_zk_text(text, &None);
    if let Ok(pd) = res {
        assert_eq!(
            pd,
            ParsedDocument::ParsedText(vec![
                DocumentComponent::new(DocumentElement::ListElement(
                    ParsedDocument::ParsedText(vec![DocumentComponent::new_text("item 1")]),
                    vec![]
                )),
                DocumentComponent::new(DocumentElement::ListElement(
                    ParsedDocument::ParsedText(vec![DocumentComponent::new_text("item 2")]),
                    vec![]
                ))
            ])
        );
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_nested_list() {
    let text = "- item 1\n    - item 1.1\n    - item 1.2\n- item 2\n    -  item 2.1";
    let res = parse_zk_text(text, &None);
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(text.replace("    ", "\t"), res);
    } else {
        panic!("Error: {res:?}");
    }
}
