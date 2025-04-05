use core::panic;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use test_log::test;

use crate::{
    document_component::Property,
    util::{apply_substitutions, indent_level, trim_like_first_line_plus},
};
use anyhow::{bail, Context, Result};
use tracing::{debug, instrument};

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
    #[regex(r"[ \t]")]
    Space,
    #[regex("\n")]
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
    #[regex("[-a-z_A-Z]+")]
    Name,
    #[token("- ")]
    ListStart,
    #[token("---")]
    FrontmatterDelim,
    /*#[regex(r"[a-zA-Z_]+\s*::=[ \t]*[a-zA-Z_]*")]
    SingleProperty,
    //#[regex(r"
    #[regex(r"[a-zA-Z_]+\s*::=[ \t]*\[(([a-zA-Z_]*\s*,\s*)*[a-zA-Z_]*)?\]")]
    MultiProperty,*/
    // the pipes are a workaround for a known bug with * in regex patterns, see e.g. https://github.com/maciejhirsz/logos/issues/456
    #[regex(r"[a-zA-Z_]+(\s|\s\s|\s\s\s|)::=\s*")]
    PropertyStart,
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"%@`~]")]
    MiscText,
    #[token("\\")]
    Backslash,
    // #[regex(r"\\u\{[a-f0-9]\}")]
    //Unicode,
    #[regex(r"[^\u0000-\u007F]+")]
    Unicode,
}

impl ZkToken {
    fn is_blank(&self) -> bool {
        matches!(self, Self::Space) || matches!(self, Self::Newline)
    }
}

pub fn parse_zk_file<T: AsRef<Path>>(file_path: T) -> Result<ParsedDocument> {
    let file_path = file_path.as_ref().canonicalize()?;
    let text =
        std::fs::read_to_string(&file_path).context("Failed to read zk file: {file_path:?}")?;

    let file_dir = file_path
        .parent()
        .context(format!("{file_path:?} has no parent!"))?
        .to_path_buf();

    let pt = parse_zk_text(&text, &Some(file_dir))
        .context(format!("Failed to parse zk file {file_path:?}"))?;
    Ok(ParsedDocument::ParsedFile(pt.into_components(), file_path))
}

#[instrument(skip_all)]
pub fn parse_zk_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use ZkToken::*;
    let text = apply_substitutions(text);
    debug!("text after subsitutions: {text:?}");

    let mut lexer = ZkToken::lexer(&text);
    let mut res = vec![];
    let mut blank_line = true;
    let indent_spaces = 0;
    // opening [ is not included as this is only run right after encountering [
    let file_link_re = regex::Regex::new(r"([-a-zäöüA-ZÄÖÜ_ /\.]+)\]\(([-a-zA-Z_/\.]+)\)")?;

    while let Some(result) = lexer.next() {
        debug!(
            "token {result:?} for '{:?}'; blank={blank_line}",
            lexer.slice()
        );
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
                        blank_line = false;
                    }
                    /*SingleProperty => {
                        let sp = parse_single_property(&lexer)?;
                        let sp = DocumentElement::Properties(vec![sp]);
                        res.push(DocumentComponent::new(sp));
                        blank_line = false;
                    }
                    MultiProperty => {
                        let mp = parse_multi_property(&lexer)?;
                        let mp = DocumentElement::Properties(vec![mp]);
                        res.push(DocumentComponent::new(mp));
                        blank_line = false;
                    }*/
                    PropertyStart => {
                        if blank_line {
                            debug!("found property start: {lexer:?}");
                            let name = lexer.slice().trim().trim_end_matches("::=").trim();
                            let prop = parse_property(&mut lexer, name.to_string(), file_dir)?;
                            res.push(DocumentComponent::new(DocumentElement::Properties(vec![
                                prop,
                            ])));
                        } else {
                            res.push(DocumentComponent::new_text(lexer.slice()));
                        }
                    }
                    SingleHash => {
                        if blank_line {
                            debug!("found heading: {lexer:?}");
                            let elem = parse_heading(&mut lexer)?;
                            let comp = DocumentComponent::new(elem);
                            res.push(comp);
                            blank_line = true;
                        } else {
                            res.push(DocumentComponent::new_text("#"));
                            blank_line = false;
                        }
                    }
                    Name => {
                        res.push(DocumentComponent::new(DocumentElement::Text(
                            lexer.slice().to_string(),
                        )));
                        blank_line = false;
                    }
                    AdNoteStart => {
                        res.push(DocumentComponent::new(parse_adnote(&mut lexer, file_dir)?));
                        blank_line = false;
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
                        blank_line = false;
                    }
                    Bracket => {
                        // check whether this is a file link
                        let remaining = lexer.remainder();
                        debug!("checking for file link: remaining: {remaining:?}");
                        if let Some(c) = file_link_re.captures(remaining) {
                            debug!("file link match!");
                            let name = c.get(1).map(|name| name.as_str().to_string());
                            let Some(path) = c.get(2) else { panic!("") };
                            let file_link = DocumentElement::FileLink(
                                MentionedFile::FilePath(PathBuf::from(path.as_str())),
                                None,
                                name,
                            );
                            let file_link = DocumentComponent::new(file_link);
                            res.push(file_link);

                            // consume tokens from the lexer until we have consumed the first
                            // closing paranthesis
                            while let Some(token) = lexer.next() {
                                if token.is_err() {
                                    bail!("Failed to consume tokens corresponding to file link. Encountered {:?}", construct_error_details(&lexer))
                                };
                                let slice = lexer.slice();
                                if slice.ends_with(')') {
                                    break;
                                } else if slice.contains(')') {
                                    bail!("No slice should contain ')', but got {slice:?}");
                                }
                            }
                        } else {
                            debug!("no file link match!");
                            res.push(DocumentComponent::new_text("["));
                        }
                        blank_line = false;
                    }
                    ClosingBracket => {
                        res.push(DocumentComponent::new_text("]"));
                        blank_line = false;
                    }
                    Backslash => {
                        res.push(DocumentComponent::new_text("\\"));
                        blank_line = false;
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
                        blank_line = false;
                    }
                    MiscText => {
                        res.push(DocumentComponent::new_text(lexer.slice()));
                        blank_line = false;
                    }
                    CarriageReturn => {
                        res.push(DocumentComponent::new_text("\r"));
                    }
                    ListStart => {
                        if blank_line {
                            let le = parse_list_element(&mut lexer, indent_spaces, file_dir)?;
                            let mut comps = le.into_components();
                            res.append(&mut comps);
                            blank_line = true;
                        } else {
                            res.push(DocumentComponent::new_text("- "));
                        }
                    }
                    FrontmatterDelim => {
                        let fm = parse_frontmatter(&mut lexer, file_dir)?;
                        res.push(fm);
                    }
                    Unicode => {
                        let slice = lexer.slice();
                        /*if let Some((_, code)) = slice.split_once('{') {
                            let code = code.trim_end_matches('}');
                            let unicode = u32::from_str_radix(code, 16)
                                .context(format!("Could not generate unicode for {slice:?}!"))?;
                            let text = char::from_u32(unicode)
                                .context(format!(
                                    "Failed to get char for unicode {unicode}, input: {slice:?}"
                                ))?
                                .to_string();
                            res.push(DocumentComponent::new_text(&text));
                        }*/
                        res.push(DocumentComponent::new_text(slice));
                    }
                    _ => {
                        debug!(
                            "Support missing token types: {token:?}. Falling back to adding text"
                        );
                        res.push(DocumentComponent::new_text(lexer.slice()));
                    }
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

#[instrument]
fn parse_property(
    lexer: &mut Lexer<'_, ZkToken>,
    name: String,
    file_dir: &Option<PathBuf>,
) -> Result<Property> {
    use ZkToken::*;
    let mut prop_val_text = String::new();
    while let Some(result) = lexer.next() {
        debug!("got {result:?} for {:?}", lexer.slice());
        let token = match result {
            Err(_) => bail!(
                "Failed to parse property! {result:?}; {}",
                construct_error_details(lexer)
            ),
            Ok(token) => token,
        };
        match token {
            Newline => {
                break;
            }
            other => {
                let txt = lexer.slice();
                if txt.ends_with('\n') {
                    prop_val_text.push_str(txt.trim_end());
                    break;
                } else if txt.contains('\n') {
                    bail!("parse property: encountered newline in the middle of slice {txt:?} for token {other:?}!")
                } else {
                    prop_val_text.push_str(txt);
                }
            }
        }
    }
    debug!("found property value text: {prop_val_text:?}");
    let prop_val_text = prop_val_text.trim();
    if prop_val_text.is_empty() {
        return Ok(Property::new(name, true, vec![]));
    } else if prop_val_text.starts_with('[') && prop_val_text.ends_with(']') {
        // TODO: check that brackets form a pair
        // multi property
        let prop_vals_text = &prop_val_text[1..prop_val_text.len().saturating_sub(1)];
        // split at commas that are not within paranthesis
        let mut values = vec![];
        let mut parenthesis_stack = vec![];
        let mut current = String::new();
        prop_vals_text.chars().for_each(|c| {
            if c == ',' {
                values.push(current.clone());
                current = String::new();
            } else {
                current.push(c);
                if c == '(' {
                    parenthesis_stack.push(')');
                } else if c == '[' {
                    parenthesis_stack.push(']');
                } else if c == '{' {
                    parenthesis_stack.push('}');
                } else if let Some(ch) = parenthesis_stack.last() {
                    if *ch == c {
                        parenthesis_stack.pop();
                    }
                }
            }
        });
        if !current.is_empty() {
            values.push(current);
        }

        return Ok(Property::new_parse(
            name,
            false,
            &values,
            crate::parse::TextMode::Zk,
            file_dir,
        ));
    } else {
        return Ok(Property::new_parse(
            name,
            true,
            &[prop_val_text.to_string()],
            crate::parse::TextMode::Zk,
            file_dir,
        ));
    }
}

fn parse_single_property(
    lexer: &Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<Property> {
    let parts = lexer
        .slice()
        .split_once("::=")
        .context("Single property must contain '::='!")?;
    let prop_name = parts.0.trim();
    let prop_value = parse_prop_values(parts.1).0;
    Ok(Property::new_parse(
        prop_name.to_string(),
        true,
        &prop_value,
        crate::parse::TextMode::Zk,
        file_dir,
    ))
}

fn parse_multi_property(
    lexer: &Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<Property> {
    let parts = lexer
        .slice()
        .split_once("::=")
        .context("Single property must contain '::='!")?;
    let prop_name = parts.0.trim();
    let prop_values = parse_prop_values(parts.1).0;
    Ok(Property::new_parse(
        prop_name.to_string(),
        false,
        &prop_values,
        crate::parse::TextMode::Zk,
        file_dir,
    ))
}

// returns vec<values>, is_multi_property (in brackets)
fn parse_prop_values(text: &str) -> (Vec<String>, bool) {
    let text = text.trim();
    let multi = text.starts_with('[') && text.ends_with(']');
    let prop_values = text.trim().replace("[", "").replace("]", "");
    (
        if prop_values.is_empty() {
            vec![]
        } else {
            prop_values
                .split(",")
                .map(|s| s.trim().to_string())
                .collect()
        },
        multi,
    )
}

#[instrument]
fn parse_frontmatter(
    lexer: &mut Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<DocumentComponent> {
    use ZkToken::*;
    let mut text = String::new();
    while let Some(result) = lexer.next() {
        debug!("got {result:?} for {:?}", lexer.slice());
        let token = match result {
            Err(_) => bail!(
                "Failed to parse frontmatter! {result:?}; {}",
                construct_error_details(lexer)
            ),
            Ok(token) => token,
        };
        match token {
            FrontmatterDelim => {
                let mut props = vec![];
                text.lines().try_for_each(|l| {
                    let tmp: anyhow::Result<()> = if l.is_empty() {
                        Ok(())
                    } else {
                        let parts = l
                            .split_once(":")
                            .context("frontmatter lines need to contain a colon, got {l:?}")?;
                        let name = parts.0.trim();
                        let (vals, is_multi) = parse_prop_values(parts.1);
                        props.push(Property::new_parse(
                            name.to_string(),
                            !is_multi,
                            &vals,
                            crate::parse::TextMode::Zk,
                            file_dir,
                        ));
                        Ok(())
                    };
                    tmp
                })?;
                return Ok(DocumentComponent::new(DocumentElement::Frontmatter(props)));
            }
            _ => {
                text.push_str(lexer.slice());
            }
        }
    }
    bail!("Reached the end of frontmatter!");
}

fn construct_error_details(lexer: &Lexer<'_, ZkToken>) -> String {
    let orig_slice = lexer.slice();
    let slice = orig_slice.escape_default().to_string();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!(
        "Encountered '{orig_slice}' ({slice:?}) at {:?} (line {line}); {lexer:?}",
        lexer.span()
    )
}

#[instrument]
fn parse_list_element(
    lexer: &mut Lexer<'_, ZkToken>,
    initial_indent_spaces: usize,
    file_dir: &Option<PathBuf>,
) -> Result<ParsedDocument> {
    // determine the extent of the list
    let mut last_was_blank = false;
    let mut blank_line = false;
    let mut text = " ".repeat(initial_indent_spaces);
    while let Some(token) = lexer.next() {
        let token = match token {
            Ok(token) => token,
            Err(_) => bail!(
                "list element parsing failed: {}",
                construct_error_details(lexer)
            ),
        };
        let slice = lexer.slice();
        debug!(
            "token: {token:?} for {slice:?}; is blank={}; last: {last_was_blank}",
            token.is_blank()
        );
        text.push_str(slice);
        if !token.is_blank() {
            blank_line = false;
        }
        if token == ZkToken::Newline {
            last_was_blank = blank_line;
            blank_line = true;
        }
        if blank_line && last_was_blank {
            debug!("finishing list text search");
            break;
        }
    }
    debug!("list text: {text:?}");
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

#[instrument]
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

#[instrument]
fn parse_list_element_from_text(text: &str, file_dir: &Option<PathBuf>) -> Result<DocumentElement> {
    let text = text.trim().replacen("- ", "", 1);
    let pd = parse_zk_text(&text, file_dir)?;
    Ok(DocumentElement::ListElement(pd, vec![]))
}

#[instrument(skip_all)]
fn consume_tokens(lexer: &mut Lexer<'_, ZkToken>) -> Result<()> {
    while let Some(result) = lexer.next() {
        debug!("{result:?}; {:?}", lexer.slice());
        let token = result.unwrap();
        if matches!(token, ZkToken::SingleHash) {
            consume_tokens(lexer).unwrap();
        }
    }
    Ok(())
}

#[instrument]
fn parse_heading(lexer: &mut Lexer<'_, ZkToken>) -> Result<DocumentElement> {
    use ZkToken::*;
    let mut level = 1;
    let mut text = String::new();
    let mut start = true;
    while let Some(result) = lexer.next() {
        debug!("got {result:?} for {:?}", lexer.slice());
        let token = match result {
            Err(_) => {
                let slice = lexer.slice();
                if slice.ends_with('\n') {
                    text.push_str(slice.trim_end());
                    let res = Ok(DocumentElement::Heading(level, text));
                    debug!("result: {res:?}");
                    return res;
                }

                bail!(
                    "Failed to parse heading! {result:?}; {}",
                    construct_error_details(lexer)
                )
            }
            Ok(token) => token,
        };
        match token {
            SingleHash => {
                if start {
                    level += 1;
                } else {
                    text.push_str(lexer.slice());
                }
            }
            Newline => {
                let res = Ok(DocumentElement::Heading(level, text));
                debug!("result: {res:?}");
                return res;
            }
            other => {
                start = false;
                let txt = lexer.slice();
                if txt.ends_with('\n') {
                    text.push_str(txt.trim_end());
                    let res = Ok(DocumentElement::Heading(level, text));
                    debug!("result: {res:?}");
                    return res;
                } else if txt.contains('\n') {
                    bail!("parse heading: encountered newline in the middle of slice {txt:?} for token {other:?}!")
                } else {
                    text.push_str(txt);
                }
            }
        }
    }
    let res = Ok(DocumentElement::Heading(level, text));
    debug!("result: {res:?}");
    res
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
        "Failed to parse adnote: Could not match '{:?}' at positions {:?}",
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
        debug!("{pd:?}");
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
    let text = "- item 1\n    - item 1.1\n    - item 1.2\n- item 2\n    - item 2.1";
    let res = parse_zk_text(text, &None);
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(text.replace("    ", "\t"), res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_curly_heading_list() {
    let text = "# {{test}}";
    let res = parse_zk_text(text, &None);
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(text.replace("    ", "\t"), res);
    } else {
        panic!("Error: {res:?}");
    }
}
#[test]
fn test_yt_template() {
    let text = "---
date: 2024-12-01 12:05:11
tags: [video, youtube]
---

# {{blabla}}
- channel::= 
- status::= inbox
";
    let res = parse_zk_text(text, &None);
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(
            text.replace("    ", "\t").replace("::=", " ::=").trim(),
            res
        );
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_curly_braces() {
    let text = "{{blabla}}";
    let res = parse_zk_text(text, &None);
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(text.replace("    ", "\t"), res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_unicode() {
    let text = "🥭🍚";
    let res = parse_zk_text(text, &None);
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let res = pd.to_zk_text(&None);
        assert_eq!(text, res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_multi_property() {
    use crate::document_component::PropValue;
    let text = "property::= [test]";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "property".to_string(),
        false,
        vec![PropValue::String("test".to_string())],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        assert_eq!("property ::= [test]", res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_multi_property_empty() {
    let text = "property::= []";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "property".to_string(),
        false,
        vec![],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        let expected = "property ::= []";
        assert_eq!(expected, res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_multi_property_single_char() {
    use crate::document_component::PropValue;
    let text = "p ::= [a]";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "p".to_string(),
        false,
        vec![PropValue::String("a".to_string())],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        assert_eq!(text.replace("    ", "\t"), res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_single_property_file_link() {
    use crate::document_component::PropValue;
    let text = "property::= [test](../test.md)";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "property".to_string(),
        true,
        vec![PropValue::FileLink(
            MentionedFile::FilePath(PathBuf::from("../test.md")),
            None,
            Some("test".to_string()),
        )],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        let expected = "property ::= [test](../test.md)";
        assert_eq!(expected, res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_multi_property_file_link() {
    use crate::document_component::PropValue;
    let text = "property::= [[test](../test.md)]";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "property".to_string(),
        false,
        vec![PropValue::FileLink(
            MentionedFile::FilePath(PathBuf::from("../test.md")),
            None,
            Some("test".to_string()),
        )],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        let expected = "property ::= [[test](../test.md)]";
        assert_eq!(expected, res);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_property_text() {
    use crate::document_component::PropValue;
    let text = "property ::= value";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::new(DocumentElement::Properties(vec![Property::new(
        "property".to_string(),
        true,
        vec![PropValue::String("value".to_string())],
    )]));
    debug!("final parse: {res:?}");
    if let Ok(pd) = res {
        let expected = ParsedDocument::ParsedText(vec![prop]);
        assert_eq!(pd, expected);
        let res = pd.to_zk_text(&None);
        assert_eq!(res, text);
    } else {
        panic!("Error: {res:?}");
    }
}
