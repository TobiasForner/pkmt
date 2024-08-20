use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::obsidian_parsing::parse_obsidian;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MentionedFile {
    FileName(String),
    FilePath(PathBuf),
}

impl Display for MentionedFile {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        use MentionedFile::*;
        let s = match self {
            FileName(name) => name.clone(),
            FilePath(path) => path.to_string_lossy().to_string(),
        };
        fmt.write_str(&s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentElement {
    Heading(u16, String),
    /// file, optional section, optional rename
    FileLink(MentionedFile, Option<String>, Option<String>),
    FileEmbed(MentionedFile, Option<String>),
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
            Text(text) => {
                *text = DocumentElement::cleanup_text(text);
            }
            Admonition(components, _) => {
                components.iter_mut().for_each(|c| c.element.cleanup());
            }
            _ => {}
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
        let file = match &self {
            FileLink(file, _, _) => file.clone(),
            FileEmbed(file, _) => file.clone(),
            _ => {
                return vec![];
            }
        };

        match file {
            MentionedFile::FileName(name) => vec![name.clone()],
            MentionedFile::FilePath(p) => {
                vec![p.file_name().unwrap().to_string_lossy().to_string()]
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
                let mut res = String::new();
                c.to_logseq_text().lines().for_each(|line| {
                    let _ = res.write_str("\t");
                    let _ = res.write_str(line);
                });
                res
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
        .replace("–", "-")
}

pub fn convert_tree(root_dir: PathBuf, target_dir: PathBuf, mode: &str) -> Result<Vec<String>> {
    let root_dir = root_dir.canonicalize()?;
    let files = files_in_tree(&root_dir, &Some(vec!["md"]))?;
    if !target_dir.exists() {
        std::fs::create_dir_all(&target_dir)?;
    }
    let target_dir = target_dir.canonicalize()?;

    let mentioned_files = files
        .iter()
        .map(|f| {
            let rel = pathdiff::diff_paths(f, &root_dir).unwrap();
            let target = target_dir.join(&rel);
            convert_file(f, &target, mode)
        })
        .collect::<Result<Vec<Vec<String>>>>();
    match mentioned_files {
        Ok(v) => Ok(v.into_iter().flat_map(|v| v.into_iter()).collect()),
        Err(e) => Err(e),
    }
}

pub fn convert_file<T: AsRef<Path> + Debug, U: AsRef<Path> + Debug>(
    file: T,
    out_file: U,
    mode: &str,
) -> Result<Vec<String>> {
    let file = file.as_ref();
    let file = file.canonicalize()?;
    let text = std::fs::read_to_string(&file)?;
    let text = apply_substitutions(&text);
    let file_dir = file
        .parent()
        .context(format!("Could get parent of {file:?}!"))?;
    let components = match mode {
        "Obsidian" => parse_obsidian(&text, &Some(file_dir.to_path_buf())),
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
    let tmp: Result<()> = dir_entry.into_iter().try_for_each(|f| {
        let path = f.unwrap().path();
        if path.is_dir() {
            let rec = files_in_tree(&path, allowed_extensions)?;
            res.extend(rec);
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
    });
    if tmp.is_err() {
        bail!("Encountered error: {tmp:?}!")
    }
    Ok(res)
}

pub fn to_logseq_text(components: &[DocumentComponent]) -> String {
    components
        .iter()
        .map(|c| c.to_logseq_text())
        .collect::<Vec<String>>()
        .join("")
        .trim()
        .to_string()
}

pub fn collapse_text(components: &[DocumentComponent]) -> Vec<DocumentComponent> {
    use DocumentElement::*;
    let mut text = String::new();
    let mut res: Vec<DocumentComponent> = vec![];
    components.iter().for_each(|c| match &c.element {
        Text(s) => {
            text.push_str(s);
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
