use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use logos::{Lexer, Logos};

use crate::{
    document_component::{
        collapse_text, DocumentComponent, DocumentElement, MentionedFile, ParsedDocument,
    },
    util::{self, indent_level, trim_like_first_line_plus},
};

fn block_ranges(text: &str) -> Vec<(usize, usize)> {
    // we use that block start only in a new line and with e sequence of whitespace followed by a dash
    let block_start = regex::Regex::new(r"(^|\r?\n\r?)(\s*-)(?:\s|$|\n)").unwrap();
    let captures = util::overlapping_captures(text, block_start.clone(), 2);

    let mut block_bounds: Vec<usize> = captures
        .iter()
        .map(|c| {
            let m = c.get(2).unwrap();
            m.start()
        })
        .collect();
    block_bounds.push(text.len());
    block_bounds.windows(2).map(|w| (w[0], w[1])).collect()
}

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

pub fn parse_logseq_text(text: &str, _file_dir: &Option<PathBuf>) -> Result<ParsedDocument> {
    let mut text = text.to_string();
    if !text.trim().starts_with('-') {
        text = format!("- {text}");
    }
    let bb = block_ranges(&text);

    let inner_block_ranges_indent: Vec<(usize, usize, usize)> = bb
        .iter()
        .map(|(s, e)| {
            let block = text[*s..*e].to_string();
            let mut start = 0;
            // remove starting new lines from blocks
            while let Some(c) = block.chars().nth(start) {
                if !['\n', '\r'].contains(&c) {
                    break;
                }
                start += 1;
            }
            let block = &block[start..];
            (*s + start, *e, indent_level(block))
        })
        .collect();
    let mut blocks = vec![];
    let mut pos = 0;
    while pos < inner_block_ranges_indent.len() {
        let (res, new_pos) = assemble_blocks_rec(pos, &inner_block_ranges_indent, &text)?;
        blocks.push(res);
        pos = new_pos;
    }
    let blocks = collapse_text(&blocks);

    let pt = ParsedDocument::ParsedText(blocks);

    Ok(pt)
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

fn parse_logseq_block(text: &str) -> Result<DocumentElement> {
    use LogSeqBlockToken::*;
    let text = text.trim();
    assert_eq!(text.chars().next(), Some('-'));

    assert!([Some(' '), Some('\n'), None].contains(&text.chars().nth(1)));
    let mut properties = vec![];
    if text.len() < 2 {
        return Ok(DocumentElement::ListElement(
            ParsedDocument::ParsedText(vec![]),
            properties,
        ));
    }
    let text = &text[2..].trim();
    let mut lexer = LogSeqBlockToken::lexer(text);
    let mut new_line_or_whitespace = true;
    let mut components = vec![];

    while let Some(result) = lexer.next() {
        if let Ok(token) = result {
            match token {
                SingleHash => {
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
                    if rec_components.len() == 1 {
                        if let DocumentElement::ListElement(pd, props) = &rec_components[0].element
                        {
                            if props.is_empty() {
                                rec_components = pd.clone().into_components();
                            }
                        }
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
    let pd = ParsedDocument::ParsedText(components);
    let list_elem = DocumentElement::ListElement(pd, properties);
    Ok(list_elem)
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
    Ok(name.trim().to_string())
}

// parses the first block in the vec together with all of its children, returns position of the
// next unhandled block
fn assemble_blocks_rec(
    pos: usize,
    block_ranges_indent: &[(usize, usize, usize)],
    text: &str,
) -> Result<(DocumentComponent, usize)> {
    let (s, e, i) = block_ranges_indent[pos];
    let block_text = &text[s..e];
    let block_text = trim_like_first_line_plus(block_text, 2);
    let first_block = parse_logseq_block(&block_text)?;
    let mut pos = pos + 1;
    let mut children = vec![];
    while pos < block_ranges_indent.len() && block_ranges_indent[pos].2 > i {
        let (child, new_pos) = assemble_blocks_rec(pos, block_ranges_indent, text)?;
        children.push(child);
        pos = new_pos;
    }

    Ok((
        DocumentComponent::new_with_children(first_block, children),
        pos,
    ))
}

fn construct_block_error_details(lexer: &Lexer<'_, LogSeqBlockToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}

#[test]
fn test_block_ranges() {
    let text = "- 23\n   - 10";
    let res = block_ranges(text);
    assert_eq!(res, vec![(0, 5), (5, 12)])
}

#[test]
fn test_simple_list_parsing() {
    use DocumentElement::*;
    use MentionedFile::FileName;
    use ParsedDocument::ParsedText;
    let text = "- first block test [[mention]]\n\t- nested block";

    let res = parse_logseq_text(text, &None).unwrap();
    let expected = ParsedText(vec![DocumentComponent::new_with_children(
        ListElement(
            ParsedText(vec![
                DocumentComponent::new_text("first block test "),
                DocumentComponent::new(FileLink(FileName("mention".to_string()), None, None)),
            ]),
            vec![],
        ),
        vec![DocumentComponent::new(ListElement(
            ParsedText(vec![DocumentComponent::new_text("nested block")]),
            vec![],
        ))],
    )]);

    assert_eq!(res, expected);

    let txt = res.to_logseq_text(&None);
    assert_eq!(txt, text);
}

#[test]
fn test_parse_properties() {
    let text = "- # Blog\n\t- template:: blog\n\t  tags:: [[blog]]\n\t  source::\n\t  description::\n\t  url::";

    let res = parse_logseq_text(text, &None).unwrap();
    //let expected = ParsedDocument::ParsedText(vec![]);

    //assert_eq!(res, expected);
    let res = res.to_logseq_text(&None);
    assert_eq!(res, text);
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

    let expected = "- ## Youtube\n\t- template:: youtube\n\t  tags:: #video, #youtube\n\t  status:: #Inbox\n\t  description::\n\t  authors::\n\t\t- [[YouTube Embed]]\n\t\t\t-\n\t\t- [[Video Notes]]\n\t\t\t-";
    assert_eq!(res, expected);
}

#[test]
fn test_comp_problem_template() {
    let text = "- # CompProblem
	- template:: computational_problem
	  tags:: [[Computational Problem]]
		- #+BEGIN_QUOTE
		  **Definition**
		  * *Input*: 
		  * *Objective*:
		  #+END_QUOTE";
    let res = parse_logseq_text(text, &None);
    assert_eq!(text, res.unwrap().to_logseq_text(&None));
}

#[test]
fn test_code_block() {
    let text = "```python\nres=set()\n```";
    let res = parse_logseq_text(text, &None).unwrap();
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
