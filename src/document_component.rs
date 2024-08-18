use std::{
    collections::HashMap,
    fmt::Debug,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::obsidian_parsing::parse_obsidian;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentElement {
    Heading(u16, String),
    /// file, optional section, optional rename
    FileLink(String, Option<String>, Option<String>),
    FileEmbed(String, Option<String>),
    Text(String),
    /// text, map storing additional properties
    Admonition(Vec<DocumentComponent>, HashMap<String, String>),
}

impl DocumentElement {
    fn to_logseq_text(&self) -> String {
        use DocumentElement::*;
        let mut tmp = self.clone();
        tmp.cleanup();
        match self {
            Heading(level, title) => {
                let title = title.trim();
                let hashes = "#".repeat(*level as usize).to_string();
                format!("- {hashes} {title}")
            }
            // todo use other parsed properties
            FileLink(file, _, _) => format!("[[{file}]]"),
            FileEmbed(file, _) => format!("{{{{embed [[{file}]]}}}}"),
            Text(text) => {
                if text.trim().is_empty() {
                    let line_count = text.lines().count();
                    if line_count >= 3 {
                        String::from("\n\n")
                    } else {
                        "\n".repeat(line_count).to_string()
                    }
                } else {
                    text.clone()
                }
            }
            Admonition(s, props) => {
                let mut parts = vec!["#+BEGIN_QUOTE".to_string()];
                if let Some(title) = props.get("title") {
                    parts.push(format!("**{title}**"));
                }
                let body = s
                    .iter()
                    .map(|c| c.to_logseq_text())
                    .collect::<Vec<String>>()
                    .join("");
                parts.push(body);
                parts.push("#+END_QUOTE".to_string());
                parts.join("\n")
            }
        }
    }

    fn cleanup(&mut self) {
        use DocumentElement::*;
        match self {
            Heading(_, text) => *text = text.trim().to_string(),
            FileLink(file, _, _) => *file = file.trim().to_string(),
            FileEmbed(file, _) => *file = file.trim().to_string(),
            Text(text) => {
                *text = DocumentElement::cleanup_text(&text);
            }
            Admonition(components, _) => {
                components.iter_mut().for_each(|c| c.element.cleanup());
            }
        }
    }

    fn cleanup_text(text: &str) -> String {
        let mut lines = vec![];
        let mut last_was_empty = false;
        text.trim().lines().for_each(|l| {
            if l.trim().is_empty() {
                last_was_empty = true;
            } else {
                if last_was_empty {
                    lines.push("");
                }
                lines.push(l);
            }
        });
        lines.join("\n")
    }

    fn mentioned_files(&self) -> Vec<String> {
        use DocumentElement::*;
        match &self {
            FileLink(file, _, _) => {
                vec![file.clone()]
            }
            FileEmbed(file, _) => {
                vec![file.clone()]
            }
            _ => {
                vec![]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentComponent {
    element: DocumentElement,
    children: Vec<Self>,
}

impl DocumentComponent {
    fn to_logseq_text(&self) -> String {
        [self.element.to_logseq_text()]
            .into_iter()
            .chain(self.children.iter().map(|c| {
                c.to_logseq_text()
                    .lines()
                    .map(|line| format!("\t{line}"))
                    .collect::<String>()
            }))
            .collect()
    }
    pub fn new(element: DocumentElement) -> Self {
        Self {
            element,
            children: vec![],
        }
    }

    pub fn new_text(text: &str) -> Self {
        Self::new(DocumentElement::Text(text.to_string()))
    }

    fn mentioned_files(&self) -> Vec<String> {
        let mut res = self.element.mentioned_files();
        res.extend(
            self.children
                .iter()
                .flat_map(|c| c.mentioned_files().into_iter()),
        );
        res
    }
}

fn apply_substitutions(text: &str) -> String {
    text.replace('−', "-")
        .replace('∗', "*")
        .replace('∈', "\\in ")
        .replace("“", "\"")
        .replace("”", "\"")
        .replace("∃", "EXISTS")
        .replace("’", "'")
        .replace("_", "_")
        .replace("–", "-")
}

pub fn convert_tree(root_dir: PathBuf, target_dir: PathBuf, mode: &str) -> Result<Vec<String>> {
    let root_dir = root_dir.canonicalize()?;
    let files = files_in_tree(&root_dir, &Some(vec!["md"]))?;
    let _ = std::fs::create_dir_all(&target_dir)?;
    let target_dir = target_dir.canonicalize()?;
    println!("target: {target_dir:?}");

    let mentioned_files = files
        .iter()
        .map(|f| {
            let rel = pathdiff::diff_paths(&f, &root_dir).unwrap();
            let target = target_dir.join(&rel);
            convert_file(f, &target, &mode)
        })
        .collect::<Result<Vec<Vec<String>>>>();
    let mentioned_files = match mentioned_files {
        Ok(v) => Ok(v.into_iter().flat_map(|v| v.into_iter()).collect()),
        Err(e) => Err(e),
    };
    mentioned_files
}

pub fn convert_file<T: AsRef<Path> + Debug, U: AsRef<Path> + Debug>(
    file: T,
    out_file: U,
    mode: &str,
) -> Result<Vec<String>> {
    let file = file.as_ref();
    let text = std::fs::read_to_string(&file)?;
    let text = apply_substitutions(&text);
    let components = match mode {
        "Obsidian" => parse_obsidian(&text),
        _ => panic!("Unsupported mode: {mode}"),
    };

    if let Ok(components) = components {
        let mentioned_files = components
            .iter()
            .flat_map(|c| c.mentioned_files().into_iter())
            .collect();
        let text = to_logseq_text(&components);

        let res =
            std::fs::write(&out_file, text).context(format!("Failed to write to {out_file:?}"));
        if res.is_err() {
            bail!("Encountered: {res:?}!");
        }
        Ok(mentioned_files)
    } else {
        bail!("Could not convert the file {file:?} to obsidian: {components:?}")
    }
}

pub fn files_in_tree<T: AsRef<Path>>(
    root_dir: T,
    allowed_extensions: &Option<Vec<&str>>,
) -> Result<Vec<PathBuf>> {
    let mut res = vec![];
    let root_dir = root_dir.as_ref().canonicalize()?;
    let dir_entry = root_dir.read_dir()?;
    let tmp: Result<()> = dir_entry
        .into_iter()
        .map(|f| {
            let path = f.unwrap().path();
            if path.is_dir() {
                let rec = files_in_tree(&path, allowed_extensions)?;
                res.extend(rec.into_iter());
            } else if let Some(ext) = path.extension() {
                if let Some(extensions) = allowed_extensions {
                    if extensions.contains(&ext.to_str().unwrap_or("should not be found")) {
                        res.push(path.clone());
                    }
                } else {
                    res.push(path.clone());
                }
            }
            Ok(())
        })
        .collect();
    if tmp.is_err() {
        bail!("Encountered error: {tmp:?}!")
    }
    Ok(res)
}

pub fn to_logseq_text(components: &Vec<DocumentComponent>) -> String {
    components
        .iter()
        .map(|c| c.to_logseq_text())
        .collect::<Vec<String>>()
        .join("")
        .trim()
        .to_string()
}

pub fn collapse_text(components: &Vec<DocumentComponent>) -> Vec<DocumentComponent> {
    use DocumentElement::*;
    let mut text = String::new();
    let mut res: Vec<DocumentComponent> = vec![];
    components.iter().for_each(|c| match &c.element {
        Text(s) => {
            text.push_str(&s);
        }
        Admonition(components, properties) => {
            if !text.is_empty() {
                res.push(DocumentComponent::new_text(&text));
                text = String::new();
            }
            let collapsed = collapse_text(components);
            res.push(DocumentComponent::new(Admonition(
                collapsed,
                properties.clone(),
            )));
        }
        _ => {
            if !text.is_empty() {
                res.push(DocumentComponent::new_text(&text));
                text = String::new();
            }
            res.push(c.clone());
        }
    });
    if !text.is_empty() {
        res.push(DocumentComponent::new_text(&text));
    }
    res
}
