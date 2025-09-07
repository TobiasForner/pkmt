use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter, Write},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use tracing::{debug, instrument};

use crate::{
    parse::{self, parse_file, TextMode},
    util::{self, ends_with_blank_line, files_in_tree, indent_level, starts_with_blank_line},
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
    pub fn to_string(&self, outmode: TextMode, file_info: &Option<FileInfo>) -> String {
        use TextMode::*;
        match outmode {
            Obsidian => todo!("Conversion to Obsidian is not implemented yet!"),
            LogSeq => self.to_logseq_text(file_info),
            Zk => self.to_zk_text(file_info),
        }
    }
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

    pub fn add_component(&mut self, component: DocumentComponent) {
        match self {
            ParsedDocument::ParsedFile(comps, _) => comps.push(component),
            ParsedDocument::ParsedText(comps) => comps.push(component),
        }
    }

    pub fn get_document_component(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<DocumentComponent> {
        for comp in self.components() {
            let rec = comp.get_document_component(selector);
            if rec.is_some() {
                return rec;
            }
        }

        None
    }

    pub fn get_all_document_components(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Vec<DocumentComponent> {
        let mut res = vec![];
        for comp in self.components() {
            let mut rec = comp.get_all_document_components(selector);
            res.append(&mut rec);
        }

        res
    }

    pub fn with_components(&self, components: Vec<DocumentComponent>) -> ParsedDocument {
        match self {
            ParsedDocument::ParsedFile(_, file_info) => {
                ParsedDocument::ParsedFile(components, file_info.to_path_buf())
            }

            ParsedDocument::ParsedText(_) => ParsedDocument::ParsedText(components),
        }
    }

    pub fn get_document_component_mut(
        &mut self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<&mut DocumentComponent> {
        use ParsedDocument::*;

        let comps = match self {
            ParsedFile(comps, _) => comps,
            ParsedText(comps) => comps,
        };
        for comp in comps.iter_mut() {
            let rec = comp.get_document_component_mut(selector);
            if rec.is_some() {
                return rec;
            }
        }

        None
    }
    pub fn _get_nth_child_mut(&mut self, n: usize) -> Option<&mut DocumentComponent> {
        match self {
            ParsedDocument::ParsedFile(comps, _) => comps.get_mut(n),
            ParsedDocument::ParsedText(comps) => comps.get_mut(n),
        }
    }
    fn mentioned_files(&self) -> Vec<String> {
        self.components()
            .iter()
            .flat_map(|c| c.mentioned_files().into_iter())
            .collect()
    }
    #[instrument]
    pub fn to_zk_text(&self, file_info: &Option<FileInfo>) -> String {
        let mut res = String::new();
        self.components().iter().for_each(|c| {
            let cblock = c.should_have_own_block();
            let text = c.to_zk_text(file_info);
            if !res.is_empty()
                && cblock
                && !ends_with_blank_line(&res)
                && !starts_with_blank_line(&text)
            {
                res.push('\n');
            }
            res.push_str(&text);
        });
        debug!("result: {res:?}");

        res
    }

    #[instrument]
    pub fn to_logseq_text(&self, file_info: &Option<FileInfo>) -> String {
        let mut res = String::new();
        let mut new_block = true;
        let mut heading_level_stack = vec![];
        self.components().iter().for_each(|c| {
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
                if !res.is_empty() && !text.starts_with('\n') {
                    if let Some((_, rest)) = res.rsplit_once("")
                        && !rest.trim().is_empty()
                    {

                    res.push('\n');
                    }
                    else{res.push('\n');}
                }
                text.lines().enumerate().for_each(|(index, line)| {
                    let mut line = line.to_string();
                    if index == 0 {
                        if !line.is_empty() {
                            res.push_str(&indent);
                        }
                        let t = text.trim();
                        // TODO: refactor with regex
                        if !(t.starts_with("- ")
                            || t.starts_with("-\n")
                            || t.starts_with("-\n\r")
                            || t == "-"
                            || line.starts_with("- "))
                        {
                           line = format!("- {line}");
                            //res.push_str("- ");
                        }
                    } else {
                        res.push('\n');
                        res.push_str(&indent);
                    }
                    debug!("removing trailing blank lines from {res:?}");
                    // remove empty lines at the end of a block
                    if line.trim().starts_with("- ") {
                        let mut first_removed = None;
                        while let Some((start, rest)) = res.rsplit_once('\n') {
                            if rest.trim().is_empty() {
                                if first_removed.is_none() {
                                    first_removed = Some(rest.to_string());
                                }
                                res = start.to_string();
                                debug!("trimmed to {res:?}");
                            } else {
                                break;
                            }
                        }
                        if let Some(first_removed) = first_removed {
                            res.push('\n');
                            res.push_str(&first_removed);
                        }
                    }
                    res.push_str(&line);
                });
            } else {
                res.push_str(&text);
            }
            new_block = c.should_have_own_block();
        });
        let res = res.trim_end().to_string();
        res
    }

    pub fn collapse_text(&self) -> Self {
        use ParsedDocument::*;
        match self {
            ParsedFile(comps, path) => ParsedFile(collapse_text(comps), path.to_path_buf()),
            ParsedText(comps) => ParsedText(collapse_text(comps)),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MentionedFile {
    FileName(String),
    FilePath(PathBuf),
}

impl MentionedFile {
    pub fn _to_mode_text(&self, file_info: &Option<FileInfo>, mode: TextMode) -> String {
        use MentionedFile::*;
        use TextMode::*;
        match mode {
            LogSeq => {
                let file_name = match self {
                    FileName(name) => name,
                    FilePath(file_path) => {
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
                                debug!("image: {file_name}: {file_info:?}");
                                let dest_dir = dest_file.parent().unwrap();
                                let rel = pathdiff::diff_paths(image_out.join(file_name), dest_dir);
                                if let Some(rel) = rel {
                                    return format!(
                                        "![{name}.{ext}]({})",
                                        rel.to_string_lossy().replace("\\", "/")
                                    );
                                } else {
                                    debug!("{image_out:?} and {dest_file:?} don't share a path!")
                                }
                            }
                        }
                    }
                }

                format!("{{{{embed [[{file_name}]]}}}}")
            }

            other => todo!("not implemented: conversion of mentioned file to {other:?}"),
        }
    }
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
pub struct Property {
    name: String,
    is_single: bool,
    pub values: Vec<PropValue>,
}

impl Property {
    pub fn to_mode_text(&self, mode: &TextMode, file_info: &Option<FileInfo>) -> String {
        use TextMode::*;
        let vals: Vec<String> = self
            .values
            .iter()
            .map(|v| v.to_mode_text(mode, file_info))
            .collect();
        match mode {
            LogSeq => {
                let value = vals.join(", ");
                // convention: if the value is whitespace-only that whitespace should be kept as is
                if value.trim().is_empty() {
                    format!("{}::{value}", self.name)
                } else {
                    format!("{}:: {value}", self.name)
                }
            }
            Zk => {
                let value = vals.join(", ");
                if self.is_single {
                    format!("{} ::= {value}", self.name)
                } else {
                    format!("{} ::= [{value}]", self.name)
                }
            }
            Obsidian => {
                todo!("not implemented: conversion of property to obsidian!")
            }
        }
    }

    fn to_zk_frontmatter_prop(&self, file_info: &Option<FileInfo>) -> String {
        let vals: Vec<String> = self
            .values
            .iter()
            .map(|v| v.to_mode_text(&TextMode::Zk, file_info))
            .collect();
        let value = vals.join(", ");
        if self.is_single {
            format!("{}: {value}", self.name)
        } else {
            format!("{}: [{value}]", self.name)
        }
    }

    pub fn new(name: String, is_single: bool, values: Vec<PropValue>) -> Self {
        Self {
            name,
            is_single,
            values,
        }
    }

    // created a new instance by parsing the passed values if possible
    pub fn new_parse(
        name: String,
        is_single: bool,
        values: &[String],
        mode: TextMode,
        file_dir: &Option<PathBuf>,
    ) -> Self {
        let values = values
            .iter()
            .map(|v| Property::try_prop_value_parse(v, &mode, file_dir))
            .collect();
        Self::new(name, is_single, values)
    }

    fn try_prop_value_parse(val: &str, mode: &TextMode, file_dir: &Option<PathBuf>) -> PropValue {
        if let Ok(pd) = parse::parse_text(val, mode, file_dir) {
            let comps = pd.components();
            if let [comp] = &comps[..] {
                if let DocumentElement::FileLink(mf, sec, rename) = &comp.element {
                    return PropValue::FileLink(mf.clone(), sec.clone(), rename.clone());
                }
            }
        }
        PropValue::String(val.to_string())
    }

    pub fn has_name(&self, name: &str) -> bool {
        self.name == name
    }

    pub fn has_value(&self, value: &PropValue) -> bool {
        self.values.iter().any(|v| v == value)
    }

    pub fn add_values(&mut self, values: &[PropValue]) {
        values.iter().for_each(|v| {
            if !self.values.contains(v) {
                self.values.push(v.clone());
            }
        });
    }
    pub fn add_values_parse(
        &mut self,
        values: &[String],
        mode: &TextMode,
        file_dir: &Option<PathBuf>,
    ) {
        values.iter().for_each(|v| {
            let v = Property::try_prop_value_parse(v, mode, file_dir);
            if !self.values.contains(&v) {
                self.values.push(v);
            }
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    String(String),
    // mentioned_file, optional section, optional rename
    FileLink(MentionedFile, Option<String>, Option<String>),
}

impl PropValue {
    pub fn to_mode_text(&self, mode: &TextMode, file_info: &Option<FileInfo>) -> String {
        use PropValue::*;
        use TextMode::*;
        match self {
            String(s) => s.to_string(),
            FileLink(mf, _section, rename) => match mode {
                LogSeq => {
                    // TODO: use section
                    format!("[[{mf}]]")
                }
                Zk => match mf {
                    MentionedFile::FilePath(p) => {
                        let mut p = p.clone();
                        if let Some(file_info) = file_info {
                            if let Some(dest) = &file_info.destination_file {
                                if let Some(parent) = dest.parent() {
                                    let rel = pathdiff::diff_paths(&p, parent);
                                    debug!("determined relative path {rel:?}");
                                    if let Some(rel) = rel {
                                        p = rel;
                                    }
                                }
                            }
                        }
                        let p = p.as_os_str();
                        let p = p.to_string_lossy();
                        if let Some(name) = rename {
                            format!("[{name}]({p})")
                        } else {
                            format!("[{p}]({p})")
                        }
                    }
                    MentionedFile::FileName(mentioned_name) => {
                        if let Some(name) = rename {
                            format!("[{name}]({mentioned_name})")
                        } else {
                            format!("[{mentioned_name}]({mentioned_name})")
                        }
                    }
                },
                other => {
                    todo!("not implemented: conversion of PropValue to {other:?}")
                }
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListElem {
    pub contents: ParsedDocument,
    pub children: Vec<ListElem>,
}

impl ListElem {
    pub fn new(contents: ParsedDocument) -> Self {
        ListElem {
            contents,
            children: vec![],
        }
    }
    pub fn to_mode_text(
        &self,
        mode: &TextMode,
        file_info: &Option<FileInfo>,
        indent_level: usize,
    ) -> String {
        let contents = match mode {
            TextMode::LogSeq => self.contents.to_logseq_text(file_info),
            TextMode::Zk => self.contents.to_zk_text(file_info),
            _ => todo!(),
        };
        let mut res = String::new();
        contents.lines().enumerate().for_each(|(i, l)| {
            if i > 0 {
                res.push('\n');
            }
            (0..indent_level).for_each(|_| res.push_str("    "));
            if i == 0 {
                if !l.starts_with("- ") {
                    res.push_str("- ");
                }
            } else {
                // indent to compensate for '- ' prefix of first line of this list element
                res.push_str("  ");
            }
            res.push_str(l);
        });
        if contents.is_empty() {
            (0..indent_level).for_each(|_| res.push_str("    "));
            res.push('-');
        }
        self.children.iter().for_each(|c| {
            let text = c.to_mode_text(mode, file_info, indent_level + 1);
            res.push('\n');
            res.push_str(&text);
        });
        res
    }

    fn collapse_text(&self) -> Self {
        let contents = ParsedDocument::ParsedText(collapse_text(self.contents.components()));
        let mut res = ListElem::new(contents);
        let children = self.children.iter().map(|c| c.collapse_text()).collect();
        res.children = children;
        res
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
    /// list_elems, terminated by blank line
    List(Vec<ListElem>, bool),

    Properties(Vec<Property>),
    Frontmatter(Vec<Property>),
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
            Frontmatter(_props) => {
                todo!("frontmatter to logseq")
            }
            Properties(props) => {
                let mut res = String::new();
                props.iter().for_each(|p| {
                    let p_text = p.to_mode_text(&TextMode::LogSeq, file_info);
                    if !res.is_empty() {
                        res.push('\n');
                    }
                    res.push_str(&p_text);
                });
                res
            }
            Heading(level, title) => {
                let title = title.trim();
                let hashes = "#".repeat(*level as usize).to_string();
                format!("{hashes} {title}")
            }
            // TODO: use other parsed properties
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
                        if let Some((_name, ext)) = file_name.rsplit_once('.') {
                            if ["png", "jpeg"].contains(&ext) {
                                debug!("image: {file_name}: {file_info:?}");
                                let dest_dir = dest_file.parent().unwrap();
                                let rel = pathdiff::diff_paths(image_out.join(file_name), dest_dir);
                                if let Some(rel) = rel {
                                    return format!(
                                        "![image.{ext}]({})",
                                        rel.to_string_lossy().replace("\\", "/")
                                    );
                                } else {
                                    debug!("{image_out:?} and {dest_file:?} don't share a path!")
                                }
                            }
                        }
                    }
                }

                format!("{{{{embed [[{file}]]}}}}")
            }
            Text(text) => {
                /*if text.trim().is_empty() {
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
                }*/
                text.to_string()
            }
            Admonition(s, props) => {
                let mut res = "#+BEGIN_QUOTE".to_string();
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
            List(list_elems, _) => list_elems
                .iter()
                .map(|le| le.to_mode_text(&TextMode::LogSeq, file_info, 0))
                .fold(String::new(), |mut acc, le_string| {
                    if !acc.is_empty() {
                        acc.push('\n');
                    }
                    acc.push_str(&le_string);
                    acc
                }),
        }
    }

    #[instrument]
    fn to_zk_text(&self, file_info: &Option<FileInfo>) -> String {
        use DocumentElement::*;
        let mut tmp = self.clone();
        tmp.cleanup();
        let res = match self {
            Frontmatter(props) => {
                let mut res = String::from("---");
                props.iter().for_each(|p| {
                    let p_text = p.to_zk_frontmatter_prop(file_info);
                    res.push('\n');
                    res.push_str(&p_text);
                });
                res.push_str("\n---");
                res
            }
            Properties(props) => {
                let mut res = String::from("");
                props.iter().for_each(|p| {
                    if !res.is_empty() {
                        res.push('\n');
                    }
                    let p_text = p.to_mode_text(&TextMode::Zk, file_info);
                    res.push_str(&p_text);
                });
                res
            }
            Heading(level, title) => {
                let title = title.trim();
                let hashes = "#".repeat(*level as usize).to_string();
                format!("{hashes} {title}")
            }
            //TODO: use other parsed properties
            FileLink(file, _, name) => {
                match file {
                    MentionedFile::FileName(mentioned_name) => {
                        if let Some(name) = name {
                            format!("[{name}]({mentioned_name})")
                        } else {
                            format!("[{mentioned_name}]({mentioned_name})")
                        }
                    }
                    MentionedFile::FilePath(p) => {
                        debug!("file link: {file:?}; {name:?}");
                        let mut p = p.clone();
                        if let Some(file_info) = file_info {
                            if let Some(dest) = &file_info.destination_file {
                                if let Some(parent) = dest.parent() {
                                    let rel = pathdiff::diff_paths(&p, parent);
                                    debug!("determined relative path {rel:?}");
                                    if let Some(rel) = rel {
                                        p = rel;
                                    }
                                }
                            }
                        }
                        let p = p.as_os_str();
                        let p = p.to_string_lossy();
                        if let Some(name) = name {
                            let sanitized_name = name.replace(['[', ']'], "");
                            format!("[{sanitized_name}]({p})")
                        } else {
                            format!("[{p}]({p})")
                        }
                    }
                }

                //format!("[[{file}]]")
            }
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
                        if let Some((_name, ext)) = file_name.rsplit_once('.') {
                            if ["png", "jpeg"].contains(&ext) {
                                debug!("image: {file_name}: {file_info:?}");
                                let dest_dir = dest_file.parent().unwrap();
                                let rel = pathdiff::diff_paths(image_out.join(file_name), dest_dir);
                                if let Some(rel) = rel {
                                    return format!(
                                        "![image.{ext}]({})",
                                        rel.to_string_lossy().replace("\\", "/")
                                    );
                                } else {
                                    debug!("{image_out:?} and {dest_file:?} don't share a path!")
                                }
                            }
                        }
                    }
                }

                format!("{{{{embed [[{file}]]}}}}")
            }
            Text(text) => text.to_string(),
            Admonition(s, props) => {
                // TODO: proper implementation, how should admonitions be represented?
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
                let text = pd.to_zk_text(file_info);
                debug!("{self:?}: inner converted to '{text:?}'.");
                let text = text.trim_start();
                let mut res = String::new();
                if !properties.is_empty() {
                    properties
                        .iter()
                        .enumerate()
                        .for_each(|(index, (key, value))| {
                            let line = if value.is_empty() {
                                format!("{key} ::=")
                            } else {
                                format!("{key} ::= {value}")
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
            List(list_elems, terminated_by_blank_line) => {
                let mut res = list_elems
                    .iter()
                    .map(|le| le.to_mode_text(&TextMode::Zk, file_info, 0))
                    .fold(String::new(), |mut acc, le_string| {
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(&le_string);
                        acc
                    });
                if *terminated_by_blank_line {
                    res.push_str("\n\n");
                }
                res
            }
        };
        debug!("result: {res:?}");
        res
    }

    pub fn get_document_component(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<DocumentComponent> {
        use DocumentElement::*;
        match self {
            Admonition(comps, _) => comps.iter().find(|c| selector(c)).cloned(),
            ListElement(pd, _) => pd.get_document_component(selector),
            _ => None,
        }
    }
    pub fn get_all_document_components(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Vec<DocumentComponent> {
        use DocumentElement::*;
        match self {
            Admonition(comps, _) => comps.iter().filter(|c| selector(c)).cloned().collect(),
            ListElement(pd, _) => pd.get_all_document_components(selector),
            _ => vec![],
        }
    }

    pub fn get_document_component_mut(
        &mut self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<&mut DocumentComponent> {
        use DocumentElement::*;
        match self {
            Admonition(comps, _) => comps.iter_mut().find(|c| selector(c)),
            ListElement(pd, _) => pd.get_document_component_mut(selector),
            _ => None,
        }
    }

    fn should_have_own_block(&self) -> bool {
        use DocumentElement::*;
        match self {
            Frontmatter(_) => true,
            Text(_) => self.is_empty_lines(),
            Heading(_, _) => true,
            Admonition(_, _) => true,
            FileEmbed(_, _) => true,
            FileLink(_, _, _) => false,
            ListElement(_, _) => true,
            CodeBlock(_, _) => true,
            Properties(_) => true,
            List(_, _) => true,
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
    pub children: Vec<Self>,
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
                    let _ = res.write_str("\n    ");
                    let _ = res.write_str(line);
                });
                res
            }))
            .collect();
        res
    }
    #[instrument]
    fn to_zk_text(&self, file_info: &Option<FileInfo>) -> String {
        let mut res = self.element.to_zk_text(file_info);
        self.children.iter().enumerate().for_each(|(i, c)| {
            let text = c.to_zk_text(file_info);
            if !starts_with_blank_line(&text)
                && (i > 0 && self.children[i - 1].should_have_own_block())
                || c.should_have_own_block()
            {
                res.push('\n');
            }
            text.lines().enumerate().for_each(|(i, l)| {
                if i > 1 {
                    res.push('\n');
                }
                if !l.is_empty() {
                    res.push('\t');
                    res.push_str(l);
                }
            });
        });
        debug!("result: {res:?}");
        res
    }

    pub fn is_empty_lines(&self) -> bool {
        self.element.is_empty_lines()
    }
    pub fn is_empty_list(&self) -> bool {
        let element_empty = match &self.element {
            DocumentElement::ListElement(pd, props) => {
                pd.components().is_empty() && props.is_empty()
            }
            _ => false,
        };
        element_empty && self.children.is_empty()
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

    pub fn get_document_component(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<DocumentComponent> {
        if selector(self) {
            return Some(self.clone());
        }
        if let Some(dc) = self.element.get_document_component(&selector) {
            return Some(dc.clone());
        }
        if let Some(dc) = self.children.iter().find(|c| selector(c)) {
            return Some(dc.clone());
        }

        None
    }
    pub fn get_all_document_components(
        &self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Vec<DocumentComponent> {
        if selector(self) {
            return vec![self.clone()];
        }
        let mut res = vec![];
        res.append(&mut self.element.get_all_document_components(&selector));

        self.children.iter().for_each(|c| {
            let mut rec = c.get_all_document_components(&selector);
            res.append(&mut rec);
        });
        res
    }

    pub fn get_element(&self) -> &DocumentElement {
        &self.element
    }

    pub fn get_element_mut(&mut self) -> &mut DocumentElement {
        &mut self.element
    }

    pub fn get_nth_child_mut(&mut self, n: usize) -> Option<&mut DocumentComponent> {
        self.children.get_mut(n)
    }

    pub fn get_document_component_mut(
        &mut self,
        selector: &dyn Fn(&DocumentComponent) -> bool,
    ) -> Option<&mut DocumentComponent> {
        if selector(self) {
            return Some(self);
        }
        if let Some(dc) = self.element.get_document_component_mut(&selector) {
            return Some(dc);
        }
        if let Some(dc) = self.children.iter_mut().find(|c| selector(c)) {
            return Some(dc);
        }

        None
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

    #[instrument]
    pub fn should_have_own_block(&self) -> bool {
        let res = self.element.should_have_own_block();
        debug!("{res}");
        res
    }
}

pub fn convert_tree(
    root_dir: PathBuf,
    target_dir: PathBuf,
    inmode: TextMode,
    outmode: TextMode,
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
            convert_file(file_info, inmode.clone(), outmode.clone())
        })
        .collect::<Result<Vec<Vec<String>>>>();
    match mentioned_files {
        Ok(v) => Ok(v.into_iter().flat_map(|v| v.into_iter()).collect()),
        Err(e) => Err(e),
    }
}

pub fn convert_file(
    file_info: FileInfo,
    inmode: TextMode,
    outmode: TextMode,
) -> Result<Vec<String>> {
    let file = &file_info.original_file;
    let pd = parse_file(file, &inmode);

    if let Ok(pd) = pd {
        let mentioned_files = pd.mentioned_files();

        let text = pd.to_string(outmode, &Some(file_info.clone()));
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
            List(list_elements, blank_line_after) => {
                let elems = list_elements.iter().map(|le| le.collapse_text()).collect();
                res.push(DocumentComponent::new(List(elems, *blank_line_after)));
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

#[ignore = "does not make sense"]
#[test]
fn test_text_comp_to_logseq() {
    let text = "line 1\n\t  line 2".to_string();
    let comp = DocumentComponent::new_text(&text);
    let res = comp.to_logseq_text(&None);

    assert_eq!(res, text)
}

#[ignore = "does not make sense"]
#[test]
fn test_text_parsed_doc_to_logseq() {
    let text = "- line 1\n\t- line 2".to_string();
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
    let expected = "\n  source::\n  description::\n  url::";
    assert_eq!(res, expected);
}

#[test]
fn test_almost_empty_pd_to_logseq() {
    use DocumentElement::ListElement;
    let pd = ParsedDocument::ParsedText(vec![DocumentComponent::new(ListElement(
        ParsedDocument::ParsedText(vec![]),
        vec![],
    ))]);
    let expected = "-";
    assert_eq!(pd.to_logseq_text(&None), expected);
}
