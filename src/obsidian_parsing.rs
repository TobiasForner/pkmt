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

pub fn parse_obsidian(text: &str) -> Result<Vec<DocumentComponent>> {
    use ObsidianToken::*;

    let mut lexer = ObsidianToken::lexer(&text);
    let mut res = vec![];

    while let Some(result) = lexer.next() {
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
                    res.push(DocumentComponent::new_text("\n"));
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
                    let parsed = parse_file_link(&mut lexer);
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
                _ => todo!("Support missing token types: {token:?}"),
            },
            Err(_) => {
                bail!("Error {}", construct_error_details(&lexer))
            }
        }
    }
    let res = collapse_text(&res);
    Ok(res)
}

fn construct_error_details(lexer: &Lexer<'_, ObsidianToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
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
            ObsidianToken::CarriageReturn => text.push_str("\r"),
            ObsidianToken::Newline => return Ok(DocumentElement::Heading(level, text)),
            ObsidianToken::Backslash => text.push_str(lexer.slice()),
            other => bail!(
                "Failed to parse heading! {other:?}: {}",
                construct_error_details(&lexer)
            ),
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
                    } else if line.starts_with("color: ") {
                        let remainder = line.strip_prefix("color: ").unwrap();
                        properties.insert("color".to_string(), remainder.trim().to_string());
                    } else {
                        body_lines.push(line);
                    }
                }
                let text = body_lines.join("\n");
                let comps = parse_obsidian(&text)?;
                return Ok(DocumentElement::Admonition(comps, properties));
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
) -> Result<(String, Option<String>, Option<String>)> {
    use ObsidianToken::*;
    let mut name = String::new();
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

    let res = parse_obsidian(text);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = vec![DocumentComponent::new(
            crate::obsidian_parsing::DocumentElement::Admonition(
                vec![DocumentComponent::new_text(
                    "Some text with $x+1$ math...\nA new line!",
                )],
                props,
            ),
        )];
        assert_eq!(res, expected);
    } else {
        assert!(false, "Got {res:?}")
    }
}

#[test]
fn test_text_parsing() {
    use DocumentElement::*;
    let text = "Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the [[MaximumClique|maximum clique problem]], unless P = NP.";
    let res = parse_obsidian(text);
    if let Ok(res) = res {
        let mut props = HashMap::new();
        props.insert("title".to_string(), "Title".to_string());
        let expected = vec![DocumentComponent::new(Text("Let $n$ denote the number of vertices in an input graph, and consider any constant $\\epsilon > 0$. Then there does not exist an $O(n^{\\epsilon-1})$-approximation algorithm for the ".to_string())), DocumentComponent::new(FileLink("MaximumClique".to_string(), None, Some("maximum clique problem".to_string()))), DocumentComponent::new(Text(", unless P = NP.".to_string()))];
        assert_eq!(res, expected);
    } else {
        assert!(false, "Got {res:?}")
    }
}

#[test]
fn test_newlines() {
    use crate::document_component::to_logseq_text;
    let text = r"														
## Basic Definitions
![[ApproximationAlgorithm]]

```ad-note
title: Theorem 
Let $n$ denote the number of vertices in an input graph, and consider any constant $\epsilon > 0$. Then there does not exist an $O(n^{\epsilon-1})$-approximation algorithm for the [[MaximumClique|maximum clique problem]], unless P = NP.
```

![[PTAS]]
";
    let res = parse_obsidian(text);
    if let Ok(components) = res {
        println!("comps: {components:?}");
        let res: String = to_logseq_text(&components);
        let expected = r"- ## Basic Definitions
{{embed [[ApproximationAlgorithm]]}}

#+BEGIN_QUOTE
**Theorem**
Let $n$ denote the number of vertices in an input graph, and consider any constant $\epsilon > 0$. Then there does not exist an $O(n^{\epsilon-1})$-approximation algorithm for the [[MaximumClique]], unless P = NP.
#+END_QUOTE

{{embed [[PTAS]]}}".to_string();
        assert_eq!(res, expected);
    } else {
        assert!(false, "Got {res:?}")
    }
}
