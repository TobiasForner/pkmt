use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::{
    parse::{parse_file, ParseMode},
    util::{files_in_tree, indent_level},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedDocument {
    ParsedFile(Vec<DocumentComponent>, PathBuf),
    ParsedText(Vec<DocumentComponent>),
}

impl ParsedDocument {
    pub fn components(&self) -> &Vec<DocumentComponent> {
        use ParsedDocument::*;
        match self {
            ParsedFile(comps, _) => comps,
            ParsedText(comps) => comps,
        }
    }
    pub fn into_components(self) -> Vec<DocumentComponent> {
        use ParsedDocument::*;
        match self {
            ParsedFile(comps, _) => comps,
            ParsedText(comps) => comps,
        }
    }

    fn mentioned_files(&self) -> Vec<String> {
        self.components()
            .iter()
            .flat_map(|c| c.mentioned_files().into_iter())
            .collect()
    }
    pub fn to_logseq_text(&self, image_dir: Option<PathBuf>) -> String {
        use ParsedDocument::*;
        let file_dirs = match self {
            ParsedFile(_, file_dir) => image_dir.map(|d| (file_dir.clone(), d)),
            ParsedText(_) => None,
        };
        let mut res = String::new();
        let mut last_empty = false;
        self.components().iter().for_each(|c| {
            if res.is_empty() && c.to_logseq_text(&file_dirs).trim().is_empty() {
                // do nothing
            } else if c.is_empty_lines() {
                if !res.is_empty() {
                    last_empty = true;
                }
            } else {
                let text = c.to_logseq_text(&file_dirs);
                if last_empty {
                    let indent = " ".repeat(indent_level(&text, 4));
                    res.push('\n');
                    if text.trim().starts_with('-') {
                        res.push_str(&text);
                    } else {
                        text.lines().enumerate().for_each(|(index, line)| {
                            if index == 0 {
                                res.push_str(&indent);
                                res.push_str("- ");
                            } else {
                                res.push('\n');
                                res.push_str(&indent);
                            }
                            res.push_str(line);
                        });
                    }
                } else {
                    res.push_str(&text);
                }
                last_empty = false;
            }
        });
        let res = res.trim_end().to_string();
        res
    }
}

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
    /// converts the element to logseq text
    /// file_dirs has the form Some(directory of the current file, directory images will be placed in) or None.
    /// If given, this information is used to update image embeds
    fn to_logseq_text(&self, file_dirs: &Option<(PathBuf, PathBuf)>) -> String {
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
            FileEmbed(file, _) => match file {
                MentionedFile::FileName(_) => format!("{{{{embed [[{file}]]}}}}"),
                MentionedFile::FilePath(p) => {
                    if let Some((file_dir, image_dir)) = file_dirs {
                        if let Some(ext) = p.extension() {
                            let ext = ext.to_string_lossy().to_string();
                            if ["png", "jpeg"].contains(&ext.as_str()) {
                                if let Some(name) = p.file_name() {
                                    let rel = pathdiff::diff_paths(image_dir.join(name), file_dir);
                                    if let Some(rel) = rel {
                                        return format!(
                                            "![image.{ext}]({})",
                                            rel.to_string_lossy().replace("\\", "/")
                                        );
                                    }
                                }
                            }
                        }
                    }
                    format!("{{{{embed [[{file}]]}}}}")
                }
            },
            Text(text) => {
                if text.trim().is_empty() {
                    let line_count = text.lines().count();
                    if line_count >= 3 {
                        String::from("\n\n")
                    } else {
                        "\n".repeat(line_count).to_string()
                    }
                } else {
                    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
                    let mut res_lines: Vec<String> = vec![];
                    let mut last_empty = false;
                    (0..lines.len()).for_each(|n| {
                        let line = &lines[n];
                        if line.is_empty() {
                            last_empty = true;
                        } else {
                            if last_empty {
                                if line.trim().starts_with('-') {
                                    res_lines.push(line.clone());
                                } else {
                                    let indent = " ".repeat(indent_level(line, 4));
                                    let line = line.trim();
                                    res_lines.push(format!("\n{indent}- {line}"));
                                }
                            } else {
                                res_lines.push(line.clone());
                            }
                            last_empty = false;
                        }
                    });
                    res_lines.join("\n")
                }
            }
            Admonition(s, props) => {
                let mut parts = vec!["#+BEGIN_QUOTE".to_string()];
                if let Some(title) = props.get("title") {
                    parts.push(format!("**{title}**"));
                }
                let body = s
                    .iter()
                    .map(|c| c.to_logseq_text(file_dirs))
                    .collect::<Vec<String>>()
                    .join("");
                parts.push(body);
                parts.push("#+END_QUOTE".to_string());
                parts.join("\n")
            }
        }
    }

    fn is_empty_lines(&self) -> bool {
        match self {
            DocumentElement::Text(text) => text.trim().is_empty() && text.lines().count() > 1,
            _ => false,
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
    /// converts the component to logseq text
    /// file_dirs has the form Some(directory of the current file, directory images will be placed in) or None.
    /// If given, this information is used to update image embeds
    fn to_logseq_text(&self, file_dirs: &Option<(PathBuf, PathBuf)>) -> String {
        [self.element.to_logseq_text(file_dirs)]
            .into_iter()
            .chain(self.children.iter().map(|c| {
                let mut res = String::new();
                c.to_logseq_text(file_dirs).lines().for_each(|line| {
                    let _ = res.write_str("\t");
                    let _ = res.write_str(line);
                });
                res
            }))
            .collect()
    }

    pub fn is_empty_lines(&self) -> bool {
        self.element.is_empty_lines()
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

pub fn convert_tree(
    root_dir: PathBuf,
    target_dir: PathBuf,
    inmode: ParseMode,
    imdir: &Option<PathBuf>,
) -> Result<Vec<String>> {
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
            convert_file(f, &target, inmode.clone(), imdir)
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
    inmode: ParseMode,
    imdir: &Option<PathBuf>,
) -> Result<Vec<String>> {
    let file = file.as_ref();
    let file = file.canonicalize()?;
    let pd = parse_file(file.clone(), inmode);

    if let Ok(pd) = pd {
        let mentioned_files = pd.mentioned_files();
        let text = pd.to_logseq_text(imdir.clone());

        let res =
            std::fs::write(&out_file, text).context(format!("Failed to write to {out_file:?}"));
        if res.is_err() {
            bail!("Encountered: {res:?}!");
        }
        Ok(mentioned_files)
    } else {
        bail!("Could not convert the file {file:?} to obsidian: {pd:?}")
    }
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
