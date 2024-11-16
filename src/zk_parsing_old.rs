use std::path::{Path, PathBuf};

use crate::obsidian_parsing::parse_obsidian_text;
use anyhow::{Context, Result};

use crate::document_component::{DocumentComponent, DocumentElement, ParsedDocument};
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
    // parse frontmatter
    let comps = if let Some((de, rest)) = parse_frontmatter(text) {
        // parse rest like obsidian
        let res = parse_obsidian_text(&rest, file_dir)?;
        // potentially process tags
        // add in the properties
        let mut comps = res.into_components();
        comps.insert(0, DocumentComponent::new(de));
        comps
    } else {
        let res = parse_obsidian_text(text, file_dir)?;
        res.into_components()
    };
    Ok(ParsedDocument::ParsedText(comps))
}

fn parse_frontmatter(text: &str) -> Option<(DocumentElement, String)> {
    let mut props: Vec<(String, Vec<String>)> = vec![];
    let mut found = false;
    let mut done = false;
    let mut rest = String::new();
    text.lines().enumerate().for_each(|(i, line)| {
        if i == 0 && line.trim() == "---" {
            found = true;
        } else if found && !done {
            if line.trim() == "---" {
                done = true;
            } else if let Some((key, values)) = line.split_once(':') {
                let values = values.trim();
                if values.starts_with('[') && values.ends_with(']') {
                    let values = &values[1..values.len() - 1];
                    let values = values.split(", ").map(|v| v.to_string()).collect();
                    props.push((key.to_string(), values));
                } else {
                    props.push((key.to_string(), vec![values.to_string()]));
                }
            } else {
                panic!("Invalid yaml frontmatter line: {line:?}");
            }
        } else if rest.is_empty() {
            rest.push_str(line);
        } else {
            rest.push('\n');
            rest.push_str(line);
        }
    });
    if found && done {
        Some((DocumentElement::Properties(props), rest))
    } else {
        None
    }
}

#[test]
fn test_zk_parsing() {
    let text = "---
date: 2024-11-17 14:46:24
tags: [test, other]
---

# test
blabla";
    let res = parse_zk_text(text, &None).unwrap();

    let expected = ParsedDocument::ParsedText(vec![
        DocumentComponent::new(DocumentElement::Properties(vec![
            ("date".to_string(), vec!["2024-11-17 14:46:24".to_string()]),
            (
                "tags".to_string(),
                vec!["test".to_string(), "other".to_string()],
            ),
        ])),
        DocumentComponent::new(DocumentElement::Heading(1, " test".to_string())),
        DocumentComponent::new(DocumentElement::Text("\nblabla".to_string())),
    ]);

    assert_eq!(res, expected);
}
