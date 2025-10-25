use core::panic;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use test_log::test;

use crate::{
    document_component::{ListElem, Property},
    parsing::md_parsing::{ListElement, MdComponent, parse_md_text},
    util::{apply_substitutions, file_link_pattern, link_name_pattern},
};
use anyhow::{Context, Result, bail};
use tracing::{debug, instrument};

use crate::document_component::{DocumentComponent, MentionedFile, ParsedDocument, collapse_text};
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
    #[regex("[-a-z_A-Z]+")]
    Name,
    #[token("- ")]
    ListStart,
    #[token("---")]
    FrontmatterDelim,
    // the pipes are a workaround for a known bug with * in regex patterns, see e.g. https://github.com/maciejhirsz/logos/issues/456
    #[regex(r"[a-zA-Z_]+(\s|\s\s|\s\s\s|)::=\s*")]
    PropertyStart,
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"%@`~]")]
    MiscText,
    #[token("\\")]
    Backslash,
    #[regex(r"[^\u0000-\u007F]+")]
    Unicode,
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

#[instrument]
pub fn parse_zk_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    let parsed_md = parse_md_text(text).context("Failed to parse md")?;
    let mut components = vec![];
    parsed_md.into_iter().try_for_each(|comp| match comp {
        MdComponent::Heading(level, text) => {
            components.push(DocumentComponent::Heading(level as u16, text));
            Ok::<(), anyhow::Error>(())
        }
        MdComponent::Text(text) => {
            let tmp = parse_zk_text_inner(&text, file_dir)?;
            let mut comps = tmp.into_components();
            components.append(&mut comps);
            Ok(())
        }
        MdComponent::List(list_elements, terminated_by_blank_line) => {
            let list_elements: Result<Vec<ListElem>> = list_elements
                .iter()
                .map(|le| parse_md_list_element(le, file_dir))
                .collect();
            components.push(DocumentComponent::List(
                list_elements?,
                terminated_by_blank_line,
            ));
            Ok(())
        }
    })?;

    Ok(ParsedDocument::ParsedText(components))
}

fn parse_md_list_element(
    list_element: &ListElement,
    file_dir: &Option<PathBuf>,
) -> Result<ListElem> {
    let contents = parse_zk_text_inner(&list_element.text, file_dir)?;
    let children: Result<Vec<ListElem>> = list_element
        .children
        .iter()
        .map(|c| parse_md_list_element(c, file_dir))
        .collect();
    let mut res = ListElem::new(contents);
    res.children = children?;
    Ok(res)
}

#[instrument(skip_all)]
pub fn parse_zk_text_inner(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use ZkToken::*;
    let text = apply_substitutions(text);
    debug!("text after subsitutions: {text:?}");

    let mut lexer = ZkToken::lexer(&text);
    let mut res = vec![];
    let mut blank_line = true;
    // opening [ is not included as this is only run right after encountering [
    let file_link_re = regex::Regex::new(&format!(
        r"{}\]\({}\)",
        link_name_pattern(),
        file_link_pattern()
    ))?;

    while let Some(result) = lexer.next() {
        debug!(
            "token {result:?} for '{:?}'; blank={blank_line}",
            lexer.slice()
        );
        match result {
            Ok(token) => {
                match token {
                    // TODO: figure out whether this is actually ever required
                    EmbedStart => {
                        let parsed = parse_file_link(&mut lexer, file_dir);
                        // no rename for file embeds
                        if let Ok((name, section, _)) = parsed {
                            res.push(DocumentComponent::FileEmbed(name, section));
                        } else {
                            panic!(
                                "Something went wrong when trying to parse file embed: {parsed:?}"
                            )
                        }
                        blank_line = false;
                    }
                    PropertyStart => {
                        if blank_line {
                            debug!("found property start: {lexer:?}");
                            let name = lexer.slice().trim().trim_end_matches("::=").trim();
                            let prop = parse_property(&mut lexer, name.to_string(), file_dir)?;
                            res.push(DocumentComponent::Properties(vec![prop]));
                        } else {
                            res.push(DocumentComponent::Text(lexer.slice().to_string()));
                        }
                    }
                    SingleHash => {
                        res.push(DocumentComponent::Text("#".to_string()));
                    }
                    Name => {
                        res.push(DocumentComponent::Text(lexer.slice().to_string()));
                        blank_line = false;
                    }
                    AdNoteStart => {
                        res.push(parse_adnote(&mut lexer, file_dir)?);
                        blank_line = false;
                    }
                    Space => {
                        res.push(DocumentComponent::Text(lexer.slice().to_string()));
                    }
                    Newline => {
                        res.push(DocumentComponent::Text("\n".to_string()));
                        blank_line = true;
                    }
                    Pipe => {
                        res.push(DocumentComponent::Text("|".to_string()));
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
                            let path = PathBuf::from_str(path.as_str())?;
                            debug!(
                                "Got name {name:?} ({:?}) and path {path:?} (regex: {file_link_re:?} ;;; pattern: {})",
                                c.get(1),
                                file_link_re.as_str()
                            );

                            let mf = if path.exists() {
                                MentionedFile::FilePath(path)
                            } else {
                                MentionedFile::FileName(
                                    path.as_os_str().to_string_lossy().to_string(),
                                )
                            };
                            let file_link = DocumentComponent::FileLink(mf, None, name);
                            debug!("Found file link {file_link:?}");
                            res.push(file_link);

                            let cap_len = c.get(0).unwrap().len();
                            let mut consumed = String::new();

                            // consume tokens from the lexer until we have consumed the
                            // closing paranthesis of the file link
                            while let Some(token) = lexer.next() {
                                if token.is_err() {
                                    bail!(
                                        "Failed to consume tokens corresponding to file link. Encountered {:?}",
                                        construct_error_details(&lexer)
                                    )
                                };

                                let slice = lexer.slice();
                                consumed.push_str(slice);
                                if consumed.len() == cap_len {
                                    break;
                                } else if consumed.len() > cap_len {
                                    bail!(
                                        "Consumed too much while parsing file link!: consumed {consumed:?}, but parsed {:?}",
                                        c.get(0).unwrap()
                                    );
                                }
                            }
                        } else {
                            debug!("no file link match!");
                            res.push(DocumentComponent::Text("[".to_string()));
                        }
                        blank_line = false;
                    }
                    ClosingBracket => {
                        res.push(DocumentComponent::Text("]".to_string()));
                        blank_line = false;
                    }
                    Backslash => {
                        res.push(DocumentComponent::Text("\\".to_string()));
                        blank_line = false;
                    }
                    MiscText => {
                        res.push(DocumentComponent::Text(lexer.slice().to_string()));
                        blank_line = false;
                    }
                    CarriageReturn => {
                        res.push(DocumentComponent::Text("\r".to_string()));
                    }
                    ListStart => {
                        res.push(DocumentComponent::Text("- ".to_string()));
                    }
                    FrontmatterDelim => {
                        let fm = parse_frontmatter(&mut lexer, file_dir)?;
                        res.push(fm);
                    }
                    Unicode => {
                        let slice = lexer.slice();
                        res.push(DocumentComponent::Text(slice.to_string()));
                    }
                    _ => {
                        debug!(
                            "Support missing token types: {token:?}. Falling back to adding text"
                        );
                        res.push(DocumentComponent::Text(lexer.slice().to_string()));
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
                    bail!(
                        "parse property: encountered newline in the middle of slice {txt:?} for token {other:?}!"
                    )
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
                } else if let Some(ch) = parenthesis_stack.last()
                    && *ch == c
                {
                    parenthesis_stack.pop();
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
            crate::parsing::TextMode::Zk,
            file_dir,
        ));
    } else {
        return Ok(Property::new_parse(
            name,
            true,
            &[prop_val_text.to_string()],
            crate::parsing::TextMode::Zk,
            file_dir,
        ));
    }
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
                            crate::parsing::TextMode::Zk,
                            file_dir,
                        ));
                        Ok(())
                    };
                    tmp
                })?;
                return Ok(DocumentComponent::Frontmatter(props));
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
fn parse_list_element_from_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ListElem> {
    // TODO: trailing spaces
    let text = text.trim_start().strip_prefix("- ").context(format!(
        "list element text has to start with \\s*- , but got {text:?}"
    ))?;
    let pd = parse_zk_text(text, file_dir)?;
    Ok(ListElem::new(pd))
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

fn parse_adnote(
    lexer: &mut Lexer<'_, ZkToken>,
    file_dir: &Option<PathBuf>,
) -> Result<DocumentComponent> {
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
                return Ok(DocumentComponent::Admonition(
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

#[instrument]
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
        let expected = ParsedDocument::ParsedText(vec![
            crate::document_component::DocumentComponent::Admonition(
                vec![DocumentComponent::Text(
                    "Some text with $x+1$ math...\nA new line!".to_string(),
                )],
                props,
            ),
        ]);
        assert_eq!(res, expected);
    } else {
        panic!("Got {res:?}")
    }
}

#[test]
fn test_text_parsing() {
    use DocumentComponent::*;
    let text = "Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the [maximum clique problem](MaximumClique.md), unless P = NP.";
    let res = parse_zk_text(text, &None);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = ParsedDocument::ParsedText(vec![Text("Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the ".to_string()), FileLink(MentionedFile::FileName("MaximumClique.md".to_string()), None, Some("maximum clique problem".to_string())), Text(", unless P = NP.".to_string())]);
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
            ParsedDocument::ParsedText(vec![DocumentComponent::List(
                vec![
                    ListElem {
                        contents: ParsedDocument::ParsedText(vec![DocumentComponent::Text(
                            "item 1".to_string()
                        )]),
                        children: vec![]
                    },
                    ListElem {
                        contents: ParsedDocument::ParsedText(vec![DocumentComponent::Text(
                            "item 2".to_string()
                        )]),
                        children: vec![]
                    }
                ],
                false
            ),])
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
        assert_eq!(text, res);
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
        assert_eq!(text, res);
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
        assert_eq!(text.replace("::=", " ::=").trim(), res);
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
    let prop = DocumentComponent::Properties(vec![Property::new(
        "property".to_string(),
        false,
        vec![PropValue::String("test".to_string())],
    )]);
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
    let prop =
        DocumentComponent::Properties(vec![Property::new("property".to_string(), false, vec![])]);
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
    let prop = DocumentComponent::Properties(vec![Property::new(
        "p".to_string(),
        false,
        vec![PropValue::String("a".to_string())],
    )]);
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
fn test_single_property_file_name() {
    use crate::document_component::PropValue;
    let text = "property::= [test](../test.md)";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::Properties(vec![Property::new(
        "property".to_string(),
        true,
        vec![PropValue::FileLink(
            MentionedFile::FileName("../test.md".to_string()),
            None,
            Some("test".to_string()),
        )],
    )]);
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
fn test_multi_property_file_name() {
    use crate::document_component::PropValue;
    let text = "property::= [[test](../test.md)]";
    let res = parse_zk_text(text, &None);
    let prop = DocumentComponent::Properties(vec![Property::new(
        "property".to_string(),
        false,
        vec![PropValue::FileLink(
            MentionedFile::FileName("../test.md".to_string()),
            None,
            Some("test".to_string()),
        )],
    )]);
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
    let prop = DocumentComponent::Properties(vec![Property::new(
        "property".to_string(),
        true,
        vec![PropValue::String("value".to_string())],
    )]);
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

#[test]
fn test_link_in_list() {
    let text =
        "- [Radtour München - Starnberger See](../../txpk-radtour-munchen-starnberger-see.md)";
    let res = parse_zk_text(text, &None).unwrap();
    let res = res.to_zk_text(&None);
    assert_eq!(res, text);
}

#[test]
fn test_text_after_list() {
    let text = "- a\n- b\n\nsome other text";
    let res = parse_zk_text(text, &None).unwrap();
    let res = res.to_zk_text(&None);
    assert_eq!(text, res);
}

#[test]
fn test_paranthesis_in_link_name() {
    let text = "[link (name)](some_file.md)";
    let res = parse_zk_text(text, &None).unwrap();
    let expected = ParsedDocument::ParsedText(vec![DocumentComponent::FileLink(
        MentionedFile::FileName("some_file.md".into()),
        None,
        Some("link (name)".to_string()),
    )]);
    assert_eq!(res, expected);
    let res = res.to_zk_text(&None);
    assert_eq!(text, res);
}

#[test]
fn test_link_with_special() {
    let text = "[Why don't movies look like *movies* anymore? | foo_bar](../../uuiv-why-dont-movies-look-like-movies-anymore.md)";
    let res = parse_zk_text(text, &None).unwrap();
    let expected = ParsedDocument::ParsedText(vec![DocumentComponent::FileLink(
        MentionedFile::FileName("../../uuiv-why-dont-movies-look-like-movies-anymore.md".into()),
        None,
        Some("Why don't movies look like *movies* anymore? | foo_bar".to_string()),
    )]);
    assert_eq!(res, expected);
    let res = res.to_zk_text(&None);
    assert_eq!(text, res);
}
