use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter, Write},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};

use crate::{
    parse::{parse_file, ParseMode},
    util::{self, files_in_tree, indent_level},
};

#[derive(Clone, Debug)]
pub struct FileInfo {
    original_file: PathBuf,
    destination_file: Option<PathBuf>,
    image_dirs: Option<(PathBuf, PathBuf)>,
}

impl FileInfo {
    /// returns Some(original_file, destination_file, image_dir) if all are set and None otherwise.
    fn get_all(&self) -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
        if let (Some(dest), Some((image_in, image_out))) =
            (&self.destination_file, &self.image_dirs)
        {
            return Some((
                self.original_file.clone(),
                dest.clone(),
                image_in.clone(),
                image_out.clone(),
            ));
        }
        None
    }

    pub fn try_new(
        original_file: PathBuf,
        destination_file: Option<PathBuf>,
        image_in_dir: Option<PathBuf>,
        image_out_dir: Option<PathBuf>,
    ) -> Result<Self> {
        match (image_in_dir, image_out_dir) {
            (Some(image_in), Some(image_out)) => Ok(FileInfo {
                original_file,
                destination_file,
                image_dirs: Some((image_in, image_out)),
            }),
            (None, None) => Ok(FileInfo {
                original_file,
                destination_file,
                image_dirs: None,
            }),
            _=>bail!("Image input directory and image output directory need to be either both set or unset, but got mixture!")
        }
    }
}

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
    pub fn to_logseq_text(&self, file_info: &Option<FileInfo>) -> String {
        let mut res = String::new();
        let mut new_block = true;
        let mut heading_level_stack = vec![];
        self.components().iter().for_each(|c| {
            //println!("{c:?}; {heading_level_stack:?}");
            let is_heading = if let DocumentElement::Heading(level, _) = c.element {
                if heading_level_stack.is_empty() {
                    heading_level_stack.push(level as usize);
                } else {
                    let level = level as usize;
                    // while last heading has higher or same level: pop from stack
                    while let Some(l) = heading_level_stack.pop() {
                        match l.cmp(&level) {
                            std::cmp::Ordering::Less => {
                                heading_level_stack.push(l);
                                break;
                            }
                            std::cmp::Ordering::Equal => {
                                break;
                            }
                            _ => {}
                        }
                    }
                    // now: level is > than last level on stack (if there is any)
                    // we simply push the new level
                    heading_level_stack.push(level);
                }
                true
            } else {
                false
            };

            let text = c.to_logseq_text(file_info);
            if text.trim().is_empty() || c.is_empty_lines() {
                // do nothing
            } else if new_block || c.should_have_own_block() {
                let hl = if is_heading {
                    heading_level_stack.len().saturating_sub(1)
                } else {
                    heading_level_stack.len()
                };
                let indent = " ".repeat(hl * util::SPACES_PER_INDENT);
                println!("new block: {c:?}; hl: {hl}; text: {text:?}");
                if !res.is_empty() && !text.starts_with('\n') {
                    res.push('\n');
                }
                text.lines().enumerate().for_each(|(index, line)| {
                    println!("line {line:?}");
                    if index == 0 {
                        if !line.is_empty() {
                            res.push_str(&indent);
                        }
                        if !text.trim().starts_with("- ") {
                            println!("trim of {text:?} did not give '- ', adding...");
                            res.push_str("- ");
                        }
                    } else {
                        res.push('\n');
                        res.push_str(&indent);
                    }
                    res.push_str(line);
                });
            } else {
                res.push_str(&text);
            }
            new_block = c.should_have_own_block();
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
    /// inner text, type string
    CodeBlock(String, Option<String>),

    /// list item, map stores additional properties
    ListElement(ParsedDocument, Vec<(String, String)>),
}

impl DocumentElement {
    /// converts the element to logseq text
    /// file_dirs has the form Some(directory of the current file, directory images will be placed in) or None.
    /// If given, this information is used to update image embeds
    fn to_logseq_text(&self, file_info: &Option<FileInfo>) -> String {
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
            FileEmbed(file, _) => {
                let file_name = match file {
                    MentionedFile::FileName(name) => name,
                    MentionedFile::FilePath(file_path) => {
                        if let Some(name) = file_path.file_name() {
                            &name.to_string_lossy()
                        } else {
                            "___nothing.txt"
                        }
                    }
                };
                if let Some(file_info) = file_info {
                    if let Some((_, dest_file, _, image_out)) = file_info.get_all() {
                        if let Some((name, ext)) = file_name.rsplit_once('.') {
                            if ["png", "jpeg"].contains(&ext) {
                                println!("image: {file_name}: {file_info:?}");
                                let dest_dir = dest_file.parent().unwrap();
                                let rel = pathdiff::diff_paths(image_out.join(file_name), dest_dir);
                                if let Some(rel) = rel {
                                    println!("{name:?}: {ext}, {rel:?}");
                                    return format!(
                                        "![image.{ext}]({})",
                                        rel.to_string_lossy().replace("\\", "/")
                                    );
                                } else {
                                    println!("{image_out:?} and {dest_file:?} don't share a path!")
                                }
                            }
                        }
                    }
                }

                format!("{{{{embed [[{file}]]}}}}")
            }
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
                                    let indent = " ".repeat(indent_level(line));
                                    let line = line.trim();
                                    res_lines.push(format!("\n{indent}- {line}"));
                                }
                            } else {
                                res_lines.push(line.clone());
                            }
                            last_empty = false;
                        }
                    });
                    let mut res = res_lines.join("\n");
                    if text.ends_with('\n') {
                        res.push('\n');
                    }
                    res
                }
            }
            Admonition(s, props) => {
                let mut res = "- #+BEGIN_QUOTE".to_string();
                if let Some(title) = props.get("title") {
                    res.push('\n');
                    res.push_str("**");
                    res.push_str(title);
                    res.push_str("**");
                }
                let body = s
                    .iter()
                    .map(|c| c.to_logseq_text(file_info))
                    .collect::<Vec<String>>()
                    .join("");
                let body = body.trim();
                res.push('\n');
                res.push_str(body);
                res.push('\n');
                res.push_str("#+END_QUOTE");
                res
            }
            CodeBlock(text, code_type) => {
                let mut res = if let Some(ct) = code_type {
                    format!("```{ct}\n")
                } else {
                    String::from("```\n")
                };
                res.push_str(text);
                res.push('\n');
                res.push_str("```");
                res
            }
            ListElement(pd, properties) => {
                let text = pd.to_logseq_text(file_info);
                let mut res = String::new();
                if !properties.is_empty() {
                    properties
                        .iter()
                        .enumerate()
                        .for_each(|(index, (key, value))| {
                            let line = if value.is_empty() {
                                format!("{key}::")
                            } else {
                                format!("{key}:: {value}")
                            };
                            if index > 0 {
                                res.push_str("\n  ");
                            } else {
                                res.push_str("- ");
                            }
                            res.push_str(&line);
                        });
                }
                text.lines().enumerate().for_each(|(i, l)| {
                    if res.is_empty() && i == 0 && !l.trim().starts_with("- ") {
                        res.push_str("- ");
                    } else if i > 0 {
                        res.push_str("\n  ");
                    }
                    res.push_str(l);
                });
                if res.is_empty() {
                    res.push('-')
                }
                res
            }
        }
    }

    fn should_have_own_block(&self) -> bool {
        use DocumentElement::*;
        match self {
            Text(_) => self.is_empty_lines(),
            Heading(_, _) => true,
            Admonition(_, _) => true,
            FileEmbed(_, _) => true,
            FileLink(_, _, _) => false,
            ListElement(_, _) => true,
            CodeBlock(_, _) => true,
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
    pub element: DocumentElement,
    children: Vec<Self>,
}

impl DocumentComponent {
    /// converts the component to logseq text
    /// If given, file_info is used to update image embeds
    fn to_logseq_text(&self, file_info: &Option<FileInfo>) -> String {
        let res = [self.element.to_logseq_text(file_info)]
            .into_iter()
            .chain(self.children.iter().map(|c| {
                let mut res = String::new();
                c.to_logseq_text(file_info).lines().for_each(|line| {
                    let _ = res.write_str("\n\t");
                    let _ = res.write_str(line);
                });
                res
            }))
            .collect();
        println!("\n{self:?} \n-->\n {res:?}");
        res
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

    pub fn new_with_children(element: DocumentElement, children: Vec<DocumentComponent>) -> Self {
        Self { element, children }
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

    fn should_have_own_block(&self) -> bool {
        self.element.should_have_own_block()
    }
}

pub fn convert_tree(
    root_dir: PathBuf,
    target_dir: PathBuf,
    inmode: ParseMode,
    image_dir: &Option<PathBuf>,
    image_out_dir: &Option<PathBuf>,
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
            let file_info = FileInfo::try_new(
                f.clone(),
                Some(target),
                image_dir.clone(),
                image_out_dir.clone(),
            )?;
            convert_file(file_info, inmode.clone())
        })
        .collect::<Result<Vec<Vec<String>>>>();
    match mentioned_files {
        Ok(v) => Ok(v.into_iter().flat_map(|v| v.into_iter()).collect()),
        Err(e) => Err(e),
    }
}

pub fn convert_file(file_info: FileInfo, inmode: ParseMode) -> Result<Vec<String>> {
    let file = &file_info.original_file;
    let pd = parse_file(file, inmode);

    if let Ok(pd) = pd {
        let mentioned_files = pd.mentioned_files();
        let text = pd.to_logseq_text(&Some(file_info.clone()));
        let dest_file = file_info
            .destination_file
            .clone()
            .context(format!("No destination file: {file_info:?}"))?;

        let res =
            std::fs::write(&dest_file, text).context(format!("Failed to write to {dest_file:?}"));
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
    components.iter().for_each(|c| {
        let children = collapse_text(&c.children);
        match &c.element {
            Text(s) => {
                text.push_str(s);
            }
            Admonition(components, properties) => {
                if !text.is_empty() {
                    res.push(DocumentComponent::new_text(&text));
                    text = String::new();
                }
                let collapsed = collapse_text(components);
                res.push(DocumentComponent::new_with_children(
                    Admonition(collapsed, properties.clone()),
                    children,
                ));
            }
            ListElement(pd, properties) => {
                if !text.is_empty() {
                    res.push(DocumentComponent::new_text(&text));
                    text = String::new();
                }

                let comps = collapse_text(pd.components());

                res.push(DocumentComponent::new_with_children(
                    ListElement(ParsedDocument::ParsedText(comps), properties.clone()),
                    children,
                ));
            }
            _ => {
                if !text.is_empty() {
                    res.push(DocumentComponent::new_text(&text));
                    text = String::new();
                }
                let mut c = c.clone();
                c.children = children;
                res.push(c);
            }
        }
    });
    if !text.is_empty() {
        res.push(DocumentComponent::new_text(&text));
    }
    res
}

#[test]
fn test_text_elem_to_logseq() {
    let text = "line 1\n\t  line 2".to_string();
    let elem = DocumentElement::Text(text.clone());
    let res = elem.to_logseq_text(&None);

    assert_eq!(res, text)
}
#[test]
fn test_text_comp_to_logseq() {
    let text = "line 1\n\t  line 2".to_string();
    let comp = DocumentComponent::new_text(&text);
    let res = comp.to_logseq_text(&None);

    assert_eq!(res, text)
}

#[test]
fn test_text_parsed_doc_to_logseq() {
    let text = "line 1\n\t  line 2".to_string();
    let pd = ParsedDocument::ParsedText(vec![DocumentComponent::new_text(&text)]);
    let res = pd.to_logseq_text(&None);

    assert_eq!(res, format!("- {text}"))
}

#[test]
fn test_list_element_to_logseq() {
    let text = "line 1\nline 2".to_string();
    let pd = ParsedDocument::ParsedText(vec![DocumentComponent::new_text(&text)]);
    let le = DocumentElement::ListElement(pd, vec![]);
    let res = le.to_logseq_text(&None);

    assert_eq!(res, format!("- line 1\n  line 2"));

    let list_elem = DocumentElement::ListElement(
        ParsedDocument::ParsedText(vec![]),
        vec![
            ("template".to_string(), "blog".to_string()),
            ("tags".to_string(), "[[blog]]".to_string()),
        ],
    );

    assert_eq!(
        list_elem.to_logseq_text(&None),
        "- template:: blog\n  tags:: [[blog]]".to_string()
    );
}

#[test]
fn test_comp_text_element_to_logseq() {
    let text = "\n  source::\n  description::\n  url::";

    let comp = DocumentComponent::new_text(text);
    let res = comp.to_logseq_text(&None);
    let expected = "\n- source::\n  description::\n  url::";
    assert_eq!(res, expected);
}
