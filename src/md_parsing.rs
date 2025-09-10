use crate::util::{SPACES_PER_INDENT, apply_substitutions};
use anyhow::{Result, bail};
use logos::{Lexer, Logos};
use test_log::test;
use tracing::{debug, instrument};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListElement {
    pub text: String,
    pub children: Vec<ListElement>,
}

impl ListElement {
    fn new() -> Self {
        ListElement {
            text: String::new(),
            children: vec![],
        }
    }

    fn new_text(text: String) -> Self {
        ListElement {
            text,
            children: vec![],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MdComponent {
    Heading(usize, String),
    /// list elements, terminated by blank line
    List(Vec<ListElement>, bool),
    Text(String),
}
impl MdComponent {
    fn new_text(text: &str) -> Self {
        MdComponent::Text(text.to_string())
    }
}

fn collapse_text(components: Vec<MdComponent>) -> Vec<MdComponent> {
    let mut res = vec![];
    let mut current_text = String::new();
    components.into_iter().for_each(|c| {
        if let MdComponent::Text(text) = c {
            current_text.push_str(&text);
        } else {
            if !current_text.is_empty() {
                res.push(MdComponent::Text(current_text.clone()));
                current_text = String::new();
            }
            res.push(c);
        }
    });
    if !current_text.is_empty() {
        res.push(MdComponent::Text(current_text.clone()));
    }
    res
}

#[derive(Logos, Debug, PartialEq, Clone)]
enum MdToken {
    #[token(r"#")]
    Hashtag,
    #[regex("[ \t]+")]
    Space,
    //#[regex("\n[ \t]+\n", priority = 10)]
    //BlankLine,
    #[regex("\n\r?")]
    Newline,
    #[regex("\r")]
    CarriageReturn,
    #[token("- ")]
    ListStart,
    #[regex(r####"[-a-zA-Z`_.{}^$><,0-9():=*&/;'+!?"|\[\]]+"####)]
    Text,
    #[token("\\")]
    Backslash,
    #[regex(r"[^\u0000-\u007F]+")]
    Unicode,
}

impl MdToken {
    fn is_blank(&self) -> bool {
        use MdToken::*;
        matches!(self, Space | CarriageReturn | Newline)
    }
}

#[instrument]
pub fn parse_md_text(text: &str) -> Result<Vec<MdComponent>> {
    use MdToken::*;
    let text = apply_substitutions(text);
    let text = text.replace("\t", &" ".repeat(SPACES_PER_INDENT));

    let mut lexer = MdToken::lexer(&text);
    let mut res = vec![];
    let mut blank_line = true;
    let mut indent_spaces = 0;
    let mut last_terminated_line;

    while let Some(result) = lexer.next() {
        debug!("{result:?}: '{:?}'", lexer.slice());
        last_terminated_line = false;
        match result {
            Ok(token) => {
                match token {
                    Space => {
                        res.push(MdComponent::new_text(lexer.slice()));
                    }
                    Newline => {
                        res.push(MdComponent::new_text(lexer.slice()));
                        blank_line = true;
                    }
                    ListStart => {
                        if blank_line {
                            let le = parse_list(&mut lexer, indent_spaces)?;
                            res.push(le);
                            // list is always terminated by a blank line
                            last_terminated_line = true;
                        } else {
                            res.push(MdComponent::new_text(lexer.slice()));
                        }
                    }

                    Hashtag => {
                        if blank_line {
                            let (heading, found) = parse_heading(&mut lexer)?;
                            println!("{heading:?}");
                            res.push(heading);
                            if found {
                                blank_line = true;
                                last_terminated_line = true;
                            }
                        } else {
                            res.push(MdComponent::new_text(lexer.slice()));
                        }
                    }
                    _ => {
                        res.push(MdComponent::new_text(lexer.slice()));
                    }
                }

                if !token.is_blank() && !last_terminated_line {
                    blank_line = false;
                } else if blank_line {
                    indent_spaces += lexer.slice().replace("\t", "    ").len();
                }
            }
            Err(_) => {
                bail!("Error: {}", construct_error_details(&lexer))
            }
        }
    }
    debug!("result: {res:?}");
    Ok(collapse_text(res))
}

/// returns Result<(heading comp, terminated by newline)>
fn parse_heading(lexer: &mut Lexer<'_, MdToken>) -> Result<(MdComponent, bool)> {
    let mut level = 1;
    while let Some(Ok(MdToken::Hashtag)) = lexer.next() {
        level += 1;
    }
    let mut start_text = lexer.slice().to_string();
    let mut found = true;
    let text = if start_text != "\n" {
        let (text, _, find) = text_until_token(MdToken::Newline, lexer, false)?;
        start_text.push_str(&text);
        found = find;
        start_text.trim().to_string()
    } else {
        String::new()
    };
    Ok((MdComponent::Heading(level, text.trim().to_string()), found))
}

/// returns (<text until token>, <text of token>, found)
#[instrument]
fn text_until_token(
    // token to search for
    token: MdToken,
    lexer: &mut Lexer<'_, MdToken>,
    // true iff running out of tokes should result in an error
    token_required: bool,
) -> Result<(String, String, bool)> {
    debug!("text_until_token start");
    let mut res = String::new();

    while let Some(result) = lexer.next() {
        println!("{result:?}: '{:?}'", lexer.slice());
        match result {
            Ok(some_token) => {
                if some_token == token {
                    return Ok((res, lexer.slice().to_string(), true));
                } else {
                    res.push_str(lexer.slice());
                }
            }
            Err(_) => {
                bail!(
                    "failed to parse until {token:?}: {}",
                    construct_error_details(lexer)
                )
            }
        }
    }
    debug!("Text until token end");
    if token_required {
        bail!(
            "Did not encounter the required {token:?}: {}",
            construct_error_details(lexer)
        );
    } else {
        Ok((res, String::new(), false))
    }
}

/// returns Result<(MdComponent, terminated by blank line)>
fn parse_list(lexer: &mut Lexer<'_, MdToken>, indent_spaces: usize) -> Result<MdComponent> {
    // TODO: merge this with le identification below
    let mut text = String::new();
    let mut blank_line = false;
    let mut terminated_by_blank_line = false;
    while let Some(token) = lexer.next() {
        match token {
            Ok(token) => {
                if !token.is_blank() {
                    blank_line = false;
                }
                if matches!(token, MdToken::Newline) {
                    if blank_line {
                        terminated_by_blank_line = true;
                        break;
                    }
                    blank_line = true;
                }
                text.push_str(lexer.slice());
            }
            Err(_) => {
                todo!()
            }
        }
    }
    debug!("list text: {text:?}");
    // indent_spaces, le
    let mut list_elements = vec![(indent_spaces, ListElement::new())];
    text.lines().enumerate().for_each(|(i, l)| {
        // valid list starts are either '- ' or just '-' if there is nothing after it in the
        // current line
        if let Some((indents, text)) = l.split_once("- ").or_else(|| {
            if let Some((indents, rest)) = l.split_once('-')
                && (rest.is_empty() || rest.starts_with('\n'))
            {
                Some((indents, rest))
            } else {
                None
            }
        }) && indents.trim().is_empty()
        {
            let indent_spaces = indents.replace("\t", "    ").len();
            let le = ListElement::new_text(text.to_string());
            list_elements.push((indent_spaces, le));
        } else if let Some((_, le)) = list_elements.last_mut() {
            if i > 0 {
                le.text.push('\n');
            }
            le.text.push_str(l);
        } else {
            let le = ListElement::new_text(l.to_string());
            list_elements.push((indent_spaces, le));
        }
    });

    // construct proper nesting
    let mut stack: Vec<(usize, ListElement)> = vec![];
    let mut pos = 0;
    let mut res = vec![];
    while let Some((lis, le)) = list_elements.get(pos) {
        if let Some((sis, _)) = stack.last() {
            if *lis > *sis {
                stack.push((*lis, le.clone()));
            } else {
                while stack.len() >= 2 && stack[stack.len() - 1].0 >= *lis {
                    let (_, last) = stack.pop().unwrap();
                    if let Some(new_last) = stack.last_mut() {
                        new_last.1.children.push(last.clone());
                    }
                }
                if stack.len() == 1 && stack[0].0 >= *lis {
                    let last = stack.pop().unwrap();
                    res.push(last.1);
                }
                stack.push((*lis, le.clone()));
            }
        } else {
            stack.push((*lis, le.clone()));
        }
        pos += 1;
    }
    while stack.len() >= 2 {
        let (_, last) = stack.pop().unwrap();
        if let Some(new_last) = stack.last_mut() {
            new_last.1.children.push(last.clone());
        }
    }
    let last = stack.pop().unwrap();
    res.push(last.1);
    Ok(MdComponent::List(res, terminated_by_blank_line))
}

fn construct_error_details(lexer: &Lexer<'_, MdToken>) -> String {
    let slice = lexer.slice().escape_default();
    let start = lexer.span().start;
    let text = lexer.source();
    let line = text[0..start].lines().count();
    format!("Encountered '{slice}' at {:?} (line {line});", lexer.span())
}

#[test]
fn test_basic_list() {
    let text = "- a\n- b";
    let result = parse_md_text(text).unwrap();
    let expected = vec![MdComponent::List(
        vec![
            ListElement::new_text("a".to_string()),
            ListElement::new_text("b".to_string()),
        ],
        false,
    )];
    assert_eq!(result, expected);
}

#[test]
fn test_multiple_headings() {
    let text = "# a\n## b\n##### c";
    let result = parse_md_text(text).unwrap();
    let expected = vec![
        MdComponent::Heading(1, "a".to_string()),
        MdComponent::Heading(2, "b".to_string()),
        MdComponent::Heading(5, "c".to_string()),
    ];
    assert_eq!(result, expected);
}

#[test]
fn test_nested_list() {
    let text = "- a\n\t- a1\n\t- a2\n- b";
    let result = parse_md_text(text).unwrap();
    let mut a_list = ListElement::new_text("a".to_string());
    a_list.children = vec![
        ListElement::new_text("a1".to_string()),
        ListElement::new_text("a2".to_string()),
    ];

    let expected = vec![MdComponent::List(
        vec![a_list, ListElement::new_text("b".to_string())],
        false,
    )];
    assert_eq!(result, expected);
}

#[test]
fn test_involved_list() {
    let text = "- a\n\t- a1\n\t- a2\n- b\n\n# Heading\nsome text";
    let result = parse_md_text(text).unwrap();
    let mut a_list = ListElement::new_text("a".to_string());
    a_list.children = vec![
        ListElement::new_text("a1".to_string()),
        ListElement::new_text("a2".to_string()),
    ];

    let expected = vec![
        MdComponent::List(vec![a_list, ListElement::new_text("b".to_string())], true),
        MdComponent::Heading(1, "Heading".to_string()),
        MdComponent::Text("some text".to_string()),
    ];
    assert_eq!(result, expected);
}

#[test]
fn test_multiline_list_element() {
    let text = "- a\n  b";
    let result = parse_md_text(text).unwrap();
    let expected = vec![MdComponent::List(
        vec![ListElement::new_text("a\n  b".to_string())],
        false,
    )];
    assert_eq!(result, expected)
}

#[test]
fn test_list_with_dash() {
    let text = "- a - b\n- c";
    let result = parse_md_text(text).unwrap();
    let expected = vec![MdComponent::List(
        vec![
            ListElement::new_text("a - b".to_string()),
            ListElement::new_text("c".to_string()),
        ],
        false,
    )];
    assert_eq!(result, expected)
}
