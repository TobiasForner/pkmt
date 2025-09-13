use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use logos::{Lexer, Logos};
use test_log::test;

use crate::{
    document_component::{
        DocumentComponent, DocumentElement, ListElem, MentionedFile, ParsedDocument, PropValue,
        Property, collapse_text,
    },
    md_parsing::{ListElement, MdComponent, parse_md_text},
};

pub fn parse_logseq_file<T: AsRef<Path>>(file_path: T) -> Result<ParsedDocument> {
    let file_path = file_path.as_ref().canonicalize()?;
    let text = std::fs::read_to_string(&file_path)?;
    let text = crate::util::apply_substitutions(&text);

    let file_dir = file_path
        .parent()
        .context(format!("{file_path:?} has no parent!"))?
        .to_path_buf();

    let pt = parse_logseq_text(&text, &Some(file_dir))?;
    Ok(ParsedDocument::ParsedFile(pt.into_components(), file_path))
}

pub fn parse_logseq_text(text: &str, file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    let parsed_md = parse_md_text(text).context("Failed to parse md")?;
    println!("{parsed_md:?}");
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
            let tmp = parse_logseq_block(&text, file_dir)?;
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

    let components = collapse_text(&components);
    Ok(ParsedDocument::ParsedText(components))
}

fn parse_md_list_element(
    list_element: &ListElement,
    file_dir: &Option<PathBuf>,
) -> Result<ListElem> {
    let contents = parse_logseq_block(&list_element.text, file_dir)?;
    let children: Result<Vec<ListElem>> = list_element
        .children
        .iter()
        .map(|c| parse_md_list_element(c, file_dir))
        .collect();
    let mut res = ListElem::new(contents);
    res.children = children?;
    Ok(res)
}

#[derive(Logos, Debug, PartialEq)]
enum LogSeqBlockToken {
    // Can be the start of a heading or part of the text
    #[token("#")]
    SingleHash,
    #[token("```")]
    TripleBackQuote,

    #[token("![[")]
    EmbedStart,
    #[token("#+BEGIN_QUOTE")]
    QuoteEnvStart,
    #[token("#+END_QUOTE")]
    QuoteEnvEnd,

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
    #[token("[")]
    Bracket,
    #[token("]")]
    ClosingBracket,
    // Or regular expressions.
    #[regex("[a-zA-Z_]+")]
    Name,
    #[token("-")]
    Minus,
    #[regex("[a-zA-Z][a-zA-Z_]*::")]
    PropertyStart,
    #[regex("[.{}^$><,0-9():=*&/;'+!?\"\\|\u{c4}\u{e4}\u{d6}\u{f6}\u{dc}\u{fc}\u{df}\u{b7}]+")]
    MiscText,
    #[token("\\")]
    Backslash,
}

fn parse_logseq_block(text: &str, _file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    use LogSeqBlockToken::*;
    let text = text.trim();
    let mut properties = vec![];
    let mut lexer = LogSeqBlockToken::lexer(text);
    let mut new_line_or_whitespace = true;
    let mut components = vec![];

    while let Some(result) = lexer.next() {
        if let Ok(token) = result {
            match token {
                SingleHash => {
                    // heading needs to be checked as logseq may have a heading inside a list
                    // element
                    if new_line_or_whitespace {
                        let (heading, rem) = parse_heading(&mut lexer);
                        components.push(DocumentComponent::new(heading));
                        components.push(DocumentComponent::new_text(&rem));
                    } else {
                        components.push(DocumentComponent::new_text("#"));
                    }
                }
                Space => components.push(DocumentComponent::new_text(lexer.slice())),
                Newline => {
                    new_line_or_whitespace = true;
                    components.push(DocumentComponent::new_text(lexer.slice()));
                }
                PropertyStart => {
                    new_line_or_whitespace = false;
                    let prop_name = lexer.slice().replace("::", "").trim().to_string();
                    let prop_val = parse_property_value(&mut lexer)?;
                    properties.push((prop_name, prop_val));
                }
                EmbedStart => {
                    new_line_or_whitespace = false;
                    let name = parse_file_mention(&mut lexer);
                    let mf = MentionedFile::FileName(name?);
                    let element = DocumentElement::FileEmbed(mf, None);
                    let comp = DocumentComponent::new(element);
                    components.push(comp);
                }
                OpenDoubleBraces => {
                    new_line_or_whitespace = false;
                    let name = parse_file_mention(&mut lexer);
                    let mf = MentionedFile::FileName(name?);
                    let element = DocumentElement::FileLink(mf, None, None);
                    let comp = DocumentComponent::new(element);
                    components.push(comp);
                }
                TripleBackQuote => {
                    new_line_or_whitespace = false;
                    let inner = text_until_token(TripleBackQuote, &mut lexer, true)?.0;

                    let (code_type, remaining) =
                        if let Some((first_line, rest)) = inner.split_once('\n') {
                            (Some(first_line.to_string()), rest)
                        } else {
                            (None, inner.as_str())
                        };
                    components.push(DocumentComponent::new(DocumentElement::CodeBlock(
                        remaining.trim().to_string(),
                        code_type,
                    )));
                }
                QuoteEnvStart => {
                    new_line_or_whitespace = false;
                    let inner = text_until_token(QuoteEnvEnd, &mut lexer, true)?.0;
                    let rec = parse_logseq_text(&inner, &None)?;

                    let mut rec_components = rec.into_components();
                    if rec_components.len() == 1
                        && let DocumentElement::List(list_elements, _) = &rec_components[0].element
                        && list_elements.len() == 1
                    {
                        rec_components = list_elements[0].contents.components().to_vec();
                    };

                    components.push(DocumentComponent::new(DocumentElement::Admonition(
                        rec_components,
                        HashMap::new(),
                    )))
                }
                _ => {
                    components.push(DocumentComponent::new_text(lexer.slice()));
                }
            }
        } else {
            bail!(
                "Encountered error: {}",
                construct_block_error_details(&lexer)
            );
        }
    }
    if !properties.is_empty() {
        let props = properties
            .iter()
            .map(|(k, v)| {
                Property::new(k.to_string(), true, vec![PropValue::String(v.to_string())])
            })
            .collect();

        let props = DocumentComponent::new(DocumentElement::Properties(props));
        components.insert(0, props);
    }
    let pd = ParsedDocument::ParsedText(components);
    Ok(pd)
}

fn parse_heading(lexer: &mut Lexer<'_, LogSeqBlockToken>) -> (DocumentElement, String) {
    let mut start = true;
    let mut text = String::new();
    let mut heading_level = 1;
    while let Some(result) = lexer.next() {
        match result {
            Ok(LogSeqBlockToken::Newline) => {
                return (
                    DocumentElement::Heading(heading_level, text.trim().to_string()),
                    lexer.slice().to_string(),
                );
            }
            Ok(LogSeqBlockToken::SingleHash) => {
                if start {
                    heading_level += 1;
                } else {
                    text.push_str(lexer.slice());
                }
            }
            Ok(_) => {
                start = false;
                text.push_str(lexer.slice());
            }
            Err(_) => panic!("Error: {}", construct_block_error_details(lexer)),
        }
    }

    (
        DocumentElement::Heading(heading_level, text.trim().to_string()),
        lexer.slice().to_string(),
    )
}

/// returns (<text until token>, <text of token>)
fn text_until_token(
    token: LogSeqBlockToken,
    lexer: &mut Lexer<'_, LogSeqBlockToken>,
    token_required: bool,
) -> Result<(String, String)> {
    let mut res = String::new();

    while let Some(result) = lexer.next() {
        match result {
            Ok(some_token) => {
                if some_token == token {
                    return Ok((res, lexer.slice().to_string()));
                } else {
                    res.push_str(lexer.slice());
                }
            }
            Err(_) => {
                bail!(
                    "failed to parse until {token:?}: {}",
                    construct_block_error_details(lexer)
                )
            }
        }
    }
    if token_required {
        bail!(
            "Did not encounter the required {token:?}: {}",
            construct_block_error_details(lexer)
        );
    } else {
        Ok((res, String::new()))
    }
}

fn text_until_newline(lexer: &mut Lexer<'_, LogSeqBlockToken>) -> Result<(String, String)> {
    text_until_token(LogSeqBlockToken::Newline, lexer, false)
}

fn parse_file_mention(lexer: &mut Lexer<'_, LogSeqBlockToken>) -> Result<String> {
    text_until_token(LogSeqBlockToken::ClosingDoubleBraces, lexer, true).map(|(name, _)| name)
}

fn parse_property_value(lexer: &mut Lexer<'_, LogSeqBlockToken>) -> Result<String> {
    let (name, _) = text_until_newline(lexer)?;
    let name = if !name.is_empty() && name.trim().is_empty() {
        " "
    } else {
        name.trim()
    }
    .to_string();
    Ok(name)
}

fn construct_block_error_details(lexer: &Lexer<'_, LogSeqBlockToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}

#[test]
fn test_simple_list_parsing() {
    use DocumentElement::*;
    use MentionedFile::FileName;
    use ParsedDocument::ParsedText;
    let text = "- first block test [[mention]]\n    - nested block";

    let res = parse_logseq_text(text, &None).unwrap();
    let expected = ParsedText(vec![DocumentComponent::new(List(
        vec![ListElem {
            contents: ParsedText(vec![
                DocumentComponent::new_text("first block test "),
                DocumentComponent::new(FileLink(FileName("mention".to_string()), None, None)),
            ]),
            children: vec![ListElem {
                contents: ParsedText(vec![DocumentComponent::new_text("nested block")]),
                children: vec![],
            }],
        }],
        false,
    ))]);

    assert_eq!(res, expected);

    let txt = res.to_logseq_text(&None);
    assert_eq!(txt, text);
}

#[test]
fn test_parse_properties() {
    let text = "- # Blog\n\t- template:: blog\n\t  tags:: [[blog]]\n\t  source::\n\t  description::\n\t  url::";

    let res = parse_logseq_text(text, &None).unwrap();
    let res = res.to_logseq_text(&None);
    assert_eq!(res, text.replace("\t", "    "));
}

#[test]
fn test_parse_multiline_list_element() {
    let text = "- line 1\n  line 2";

    let res = parse_logseq_text(text, &None).unwrap();
    let res = res.to_logseq_text(&None);

    assert_eq!(res, text);
}

#[test]
fn test_parse_youtube_template() {
    let text = "## Youtube\n\t- template:: youtube\n\t  tags:: #video, #youtube\n\t  status:: #Inbox\n\t  description:: \n\t  authors::\n\t\t- [[YouTube Embed]]\n\t\t\t-\n\t\t- [[Video Notes]]\n\t\t\t-";

    let res = parse_logseq_text(text, &None).unwrap();

    let res = res.to_logseq_text(&None);

    let expected = "- ## Youtube\n\t- template:: youtube\n\t  tags:: #video, #youtube\n\t  status:: #Inbox\n\t  description:: \n\t  authors::\n\t\t- [[YouTube Embed]]\n\t\t\t-\n\t\t- [[Video Notes]]\n\t\t\t-";
    assert_eq!(res, expected.replace("\t", "    "));
}

#[test]
fn test_comp_problem_template() {
    let text = "- # CompProblem\n\t- template:: computational_problem\n\t  tags:: [[Computational Problem]]\n\t\t- #+BEGIN_QUOTE\n\t\t  **Definition**\n\t\t  * *Input*: \n\t\t  * *Objective*:\n\t\t  #+END_QUOTE";
    let res = parse_logseq_text(text, &None);
    println!("{res:?}");
    assert_eq!(
        res.unwrap().to_logseq_text(&None),
        text.replace("\t", "    ")
    );
}

#[test]
fn test_code_block() {
    let text = "```python\nres=set()\n```";
    let res = parse_logseq_text(text, &None).unwrap();
    println!("{res:?}");
    let res = res.to_logseq_text(&None);
    let expected = "- ```python\n  res=set()\n  ```";
    assert_eq!(res, expected);
}
#[test]
fn test_umlaut() {
    let text = "üÜäÄöÖß";
    let res = parse_logseq_text(text, &None);
    let res = res.unwrap().to_logseq_text(&None);
    let expected = "- üÜäÄöÖß";
    assert_eq!(res, expected);
}
