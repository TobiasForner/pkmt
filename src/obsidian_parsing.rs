use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use test_log::test;
use tracing::{debug, instrument};

use crate::{
    document_component::ListElem,
    md_parsing::{ListElement, MdComponent, parse_md_text},
    util::apply_substitutions,
};
use anyhow::{Context, Result, bail};

use crate::document_component::{
    DocumentComponent, DocumentElement, MentionedFile, ParsedDocument, collapse_text,
};
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
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"]+")]
    MiscText,
    #[token("\\")]
    Backslash,
}

pub fn parse_obsidian_file<T: AsRef<Path>>(file_path: T) -> Result<ParsedDocument> {
    let file_path = file_path.as_ref().canonicalize()?;
    let text = std::fs::read_to_string(&file_path)?;

    let file_dir = file_path
        .parent()
        .context(format!("{file_path:?} has no parent!"))?
        .to_path_buf();

    let pt = parse_obsidian_text(&text, &Some(file_dir))?;
    Ok(ParsedDocument::ParsedFile(pt.into_components(), file_path))
}

#[instrument]
pub fn parse_obsidian_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    let parsed_md = parse_md_text(text).context("Failed to parse md")?;
    let mut components = vec![];
    parsed_md.into_iter().try_for_each(|comp| match comp {
        MdComponent::Heading(level, text) => {
            components.push(DocumentComponent::new(DocumentElement::Heading(
                level as u16,
                text,
            )));
            Ok::<(), anyhow::Error>(())
        }
        MdComponent::Text(text) => {
            let tmp = parse_obsidian_text_inner(&text, file_dir)?;
            let mut comps = tmp.into_components();
            components.append(&mut comps);
            Ok(())
        }
        MdComponent::List(list_elements, terminated_by_blank_line) => {
            let list_elements: Result<Vec<ListElem>> = list_elements
                .iter()
                .map(|le| parse_md_list_element(le, file_dir))
                .collect();
            components.push(DocumentComponent::new(DocumentElement::List(
                list_elements?,
                terminated_by_blank_line,
            )));
            Ok(())
        }
    })?;

    Ok(ParsedDocument::ParsedText(components))
}

fn parse_md_list_element(
    list_element: &ListElement,
    file_dir: &Option<PathBuf>,
) -> Result<ListElem> {
    let contents = parse_obsidian_text_inner(&list_element.text, file_dir)?;
    let children: Result<Vec<ListElem>> = list_element
        .children
        .iter()
        .map(|c| parse_md_list_element(c, file_dir))
        .collect();
    let mut res = ListElem::new(contents);
    res.children = children?;
    Ok(res)
}

#[instrument]
pub fn parse_obsidian_text_inner(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use ObsidianToken::*;
    let text = apply_substitutions(text);

    let mut lexer = ObsidianToken::lexer(&text);
    let mut res = vec![];

    while let Some(result) = lexer.next() {
        println!("{result:?}: '{:?}'", lexer.slice());
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
                    SingleHash => {
                        res.push(DocumentComponent::new_text(lexer.slice()));
                    }
                    Name => {
                        res.push(DocumentComponent::new(DocumentElement::Text(
                            lexer.slice().to_string(),
                        )));
                    }
                    AdNoteStart => {
                        res.push(DocumentComponent::new(parse_adnote(&mut lexer, file_dir)?));
                    }
                    Space => {
                        res.push(DocumentComponent::new(DocumentElement::Text(
                            lexer.slice().to_string(),
                        )));
                    }
                    Newline => {
                        if let Some(c) = res.last()
                            && !c.should_have_own_block()
                        {
                            res.push(DocumentComponent::new(DocumentElement::Text(
                                "\n".to_string(),
                            )));
                        }
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
                    MiscText => {
                        res.push(DocumentComponent::new_text(lexer.slice()));
                    }
                    CarriageReturn => {
                        res.push(DocumentComponent::new_text("\r"));
                    }
                    _ => todo!("Support missing token types: {token:?}"),
                }
            }
            Err(_) => {
                bail!("Error {}", construct_error_details(&lexer))
            }
        }
    }
    let res = ParsedDocument::ParsedText(collapse_text(&res));
    debug!("result: {res:?}");
    Ok(res)
}

fn construct_error_details(lexer: &Lexer<'_, ObsidianToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}

fn parse_adnote(
    lexer: &mut Lexer<'_, ObsidianToken>,
    file_dir: &Option<PathBuf>,
) -> Result<DocumentElement> {
    let mut text = String::new();
    while let Some(Ok(token)) = lexer.next() {
        match token {
            ObsidianToken::TripleBackQuote => {
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
                let pd = parse_obsidian_text(&body_text, file_dir)?;
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
    lexer: &mut Lexer<'_, ObsidianToken>,
    file_dir: &Option<PathBuf>,
) -> Result<(MentionedFile, Option<String>, Option<String>)> {
    use ObsidianToken::*;
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

    let res = parse_obsidian_text(text, &None);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = ParsedDocument::ParsedText(vec![DocumentComponent::new(
            crate::obsidian_parsing::DocumentElement::Admonition(
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
    let res = parse_obsidian_text(text, &None);
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
fn test_newlines() {
    let text = "\t\t\t\t\n## Basic Definitions
![[ApproximationAlgorithm]]

```ad-note
title: Theorem 
Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the [[MaximumClique|maximum clique problem]], unless P = NP.
```

![[PTAS]]
";
    let res = parse_obsidian_text(text, &None);
    if let Ok(res) = res {
        let res = res.to_logseq_text(&None);
        let expected = r"- ## Basic Definitions
    - {{embed [[ApproximationAlgorithm]]}}
    - #+BEGIN_QUOTE
      **Theorem**
      Let $n$ denote the number of vertices in an input graph, and consider any constant $\epsilon > 0$. Then there does not exist an $O(n^{\epsilon-1})$-approximation algorithm for the [[MaximumClique]], unless P = NP.
      #+END_QUOTE
    - {{embed [[PTAS]]}}".to_string();
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
    let res = parse_obsidian_text(test_text, &None);
    if let Ok(pd) = res {
        println!("{pd:?}");
        let logseq_text = pd.to_logseq_text(&None);
        let expected_text = "- variables will become nonzero, so we only need to keep track of these nonzero variables.\n- #### Initial Algorithm".to_string();
        assert_eq!(logseq_text, expected_text);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_admonition_in_between() {
    let test_text= "This leads to the following observation.
```ad-note
title: Observation 7.2
For any path $P$ of vertices of degree two in graph $G$, Algorithm 7.2 will choose at most one vertex from $P$; that is, $|S \\cap P| \\leq 1$ for the final solution $S$ given by the algorithm.
```
##### *Proof*
Once $S$ contains a vertex ";
    let res = parse_obsidian_text(test_text, &None);
    if let Ok(pd) = res {
        println!("{pd:?}");
        let logseq_text = pd.to_logseq_text(&None);
        let expected_text = "- This leads to the following observation.\n- #+BEGIN_QUOTE\n  **Observation 7.2**\n  For any path $P$ of vertices of degree two in graph $G$, Algorithm 7.2 will choose at most one vertex from $P$; that is, $|S \\cap P| \\leq 1$ for the final solution $S$ given by the algorithm.\n  #+END_QUOTE\n- ##### *Proof*\n    - Once $S$ contains a vertex".to_string();
        assert_eq!(logseq_text, expected_text);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_simple_list() {
    let text = "- item 1\n- item 2";
    let res = parse_obsidian_text(text, &None);
    let expected = ParsedDocument::ParsedText(vec![DocumentComponent::new(DocumentElement::List(
        vec![
            ListElem {
                contents: ParsedDocument::ParsedText(vec![DocumentComponent::new_text("item 1")]),
                children: vec![],
            },
            ListElem {
                contents: ParsedDocument::ParsedText(vec![DocumentComponent::new_text("item 2")]),
                children: vec![],
            },
        ],
        false,
    ))]);
    if let Ok(pd) = res {
        assert_eq!(pd, expected);
    } else {
        panic!("Error: {res:?}");
    }
}

#[test]
fn test_nested_list() {
    let text = "- item 1\n    - item 1.1\n    - item 1.2\n- item 2\n    -  item 2.1";
    let res = parse_obsidian_text(text, &None);
    if let Ok(pd) = res {
        let res = pd.to_logseq_text(&None);
        assert_eq!(text, res);
    } else {
        panic!("Error: {res:?}");
    }
}
