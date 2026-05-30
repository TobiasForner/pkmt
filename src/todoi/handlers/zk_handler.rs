use std::{fmt::Debug, fs::DirEntry, path::PathBuf, str::FromStr, vec};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, instrument};

use crate::convert::FileInfo;
use crate::todoi::{TaskData, handlers::TaskDataHandler};
use crate::{
    document_component::{DocumentComponent, ListElem, MentionedFile, ParsedDocument, PropValue},
    parsing::{TextMode, parse_file, zk_parsing},
};

#[derive(Debug)]
pub struct ZkHandler {
    root_dir: PathBuf,
}

impl ZkHandler {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    #[instrument]
    fn get_zk_file(
        &self,
        title: &str,
        template_path: PathBuf,
        create_missing: bool,
    ) -> Result<PathBuf> {
        use std::process::Command;
        debug!("trying to get zk file for {title}");

        let title = ZkHandler::normalize_title(title);

        let root_dir = self.root_dir.to_str().context(format!(
            "Failed to convert zk root directory to string: {:?}",
            self.root_dir
        ))?;

        let zk_list_args = [
            "list",
            "--no-input",
            "-m",
            &title,
            "-f",
            "{{title}}||||{{path}}",
            "-q",
            root_dir,
        ];

        let output = Command::new("zk")
            .args(zk_list_args)
            .output()
            .context(format!("failed to retrieve zk files for {title}"))?;
        if !output.status.success() {
            println!(
                "Failed to list zk files for title {title:?}! command: {zk_list_args:?}; {}; {}",
                str::from_utf8(&output.stdout).unwrap(),
                str::from_utf8(&output.stderr).unwrap()
            );
            bail!("Could not list zk files for {title:?}");
        }

        let found = str::from_utf8(&output.stdout)?
            .lines()
            .filter_map(|l| l.split_once("||||"))
            .find(|(found_title, _)| title == *found_title);
        if let Some((_, path_end)) = found {
            return Ok(self.root_dir.join(path_end));
        }

        if create_missing {
            let zk_args = [
                "new",
                "--no-input",
                "--title",
                &title,
                "--template",
                template_path.to_str().context(format!(
                    "Failed to convert zk template path to string: {:?}",
                    self.root_dir
                ))?,
                "--notebook-dir",
                self.root_dir.to_str().context(format!(
                    "Failed to convert zk root directory to string: {:?}",
                    self.root_dir
                ))?,
                "--notebook-dir",
                root_dir,
                "--working-dir",
                root_dir,
                "-p",
            ];
            let output = Command::new("zk")
                .args(zk_args)
                .output()
                .context(format!("failed to retrieve zk file for {title}"))?;
            if !output.status.success() {
                println!(
                    "Failed to create zk file for title {title:?}! command: {zk_args:?}; {}; {}",
                    str::from_utf8(&output.stdout).unwrap(),
                    str::from_utf8(&output.stderr).unwrap()
                );
                bail!("Could not create zk file for {title:?}");
            }
            let p = std::str::from_utf8(&output.stdout)?;
            Ok(PathBuf::from_str(p.trim())?)
        } else {
            bail!("Did not find zk file.")
        }
    }

    #[instrument]
    fn get_zk_journal_file() -> Result<PathBuf> {
        use std::process::Command;
        let output = Command::new("zk").arg("daily-path").output()?;
        let p = std::str::from_utf8(&output.stdout)?.trim();
        debug!("daily path: {p:?}");
        Ok(PathBuf::from_str(p)?)
    }

    fn fill_in_creator(
        &self,
        pd: &mut ParsedDocument,
        author: &str,
        prop_name: &str,
        file_dir: &Option<PathBuf>,
    ) -> Result<bool> {
        let file = self.get_file_for_creator(author, true)?;
        debug!("Found creator file {file:?} for {author:?}");
        self.fill_props(
            pd,
            prop_name,
            &[PropValue::FileLink(
                MentionedFile::FilePath(file),
                None,
                Some(author.to_string()),
            )],
            file_dir,
        );
        Ok(true)
    }

    #[instrument()]
    fn add_to_zk_pd(
        &self,
        pd: &mut ParsedDocument,
        task_data: &TaskData,
        file_dir: &Option<PathBuf>,
    ) -> bool {
        let frontmatter =
            pd.get_document_component_mut(&|dc| matches!(dc, DocumentComponent::Frontmatter(_)));

        let tags_to_add: Vec<String> = task_data
            .get_tags()
            .iter()
            .map(|t| t.trim_start_matches('#').to_string())
            .collect();

        let tags_success = if let Some(dc) = frontmatter {
            if let DocumentComponent::Frontmatter(properties) = dc {
                for p in properties {
                    if p.has_name("tags") {
                        p.add_values_parse(&tags_to_add, &TextMode::Zk, file_dir);
                    }
                }
                true
            } else {
                println!(
                    "Failed to find tags in template: {}",
                    pd.to_string(&TextMode::Zk, &None)
                );
                false
            }
        } else {
            println!("Failed to find frontmatter in template: {pd:?}",);
            false
        };
        if tags_success {
            match task_data {
                TaskData::Sbs(url, author, _, _, desc) => {
                    self.fill_property(pd, "url", std::slice::from_ref(url), file_dir);
                    let success = self.fill_in_creator(pd, "sbs", "source", file_dir);
                    if success.is_err() {
                        return false;
                    }
                    if let Some(author) = author {
                        let success = self.fill_in_creator(pd, author, "source", file_dir);
                        if success.is_err() {
                            return false;
                        }
                    }
                    if let Some(desc) = desc {
                        self.fill_property(pd, "description", std::slice::from_ref(desc), file_dir);
                    }
                }
                TaskData::Reddit(url, _, _) => {
                    self.fill_property(pd, "url", std::slice::from_ref(url), file_dir);
                }
                TaskData::Youtube(url, title, channel, _) => {
                    self.fill_property(pd, "url", std::slice::from_ref(url), file_dir);
                    let success = self.fill_in_creator(pd, channel, "channel", file_dir);
                    if success.is_err() {
                        println!("Could not fill in creator for {url:?}: {success:?}");
                        return false;
                    }
                    self.fill_property(pd, "description", std::slice::from_ref(title), file_dir);
                }
                TaskData::YtPlaylist(url, channel, _) => {
                    self.fill_property(pd, "url", std::slice::from_ref(url), file_dir);
                    let success = self.fill_in_creator(pd, channel, "channel", file_dir);
                    if success.is_err() {
                        return false;
                    }
                }
                TaskData::Unhandled => {
                    return false;
                }
                TaskData::Interactive(_, url, _, _, sources) => {
                    if let Some(url) = url {
                        debug!("filled in url");
                        self.fill_property(pd, "url", std::slice::from_ref(url), file_dir);
                    }
                    sources.iter().for_each(|s| {
                        let _ = self.fill_in_creator(pd, s, "source", file_dir);
                    });
                }
            }
            return true;
        }
        false
    }

    #[instrument]
    fn append_to_zk_journal(&self, dc: DocumentComponent) -> Result<bool> {
        let journal_path = ZkHandler::get_zk_journal_file()?;
        let mut pd = parse_file(&journal_path, &TextMode::Zk)?;
        debug!("adding {dc:?} to journal file");
        pd.add_component(dc);
        let file_info = FileInfo::new_same_file(journal_path.clone());
        let journal_text = pd.to_zk_text(&Some(file_info));
        debug!("new journal text: {journal_text:?}");

        std::fs::write(&journal_path, journal_text)
            .context(format!("Could not write file {journal_path:?}"))?;
        Ok(true)
    }

    #[instrument]
    fn fill_property(
        &self,
        pd: &mut ParsedDocument,
        prop_name: &str,
        values: &[String],
        file_dir: &Option<PathBuf>,
    ) {
        let property = pd.get_document_component_mut(&|dc| match dc {
            DocumentComponent::Properties(props) => props.iter().any(|p| p.has_name(prop_name)),
            _ => false,
        });
        if let Some(prop) = property
            && let DocumentComponent::Properties(props) = prop
        {
            props.iter_mut().for_each(|p| {
                if p.has_name(prop_name) {
                    p.add_values_parse(values, &TextMode::Zk, file_dir);
                }
            });
        }
    }

    /// Adds the the given values to the first property in the pd with the given name. Does nothing if the property
    /// is not found
    #[instrument]
    fn fill_props(
        &self,
        pd: &mut ParsedDocument,
        prop_name: &str,
        values: &[PropValue],
        file_dir: &Option<PathBuf>,
    ) {
        let property = pd.get_document_component_mut(&|dc| match dc {
            DocumentComponent::Properties(props) => props.iter().any(|p| p.has_name(prop_name)),
            _ => false,
        });
        if let Some(prop) = property
            && let DocumentComponent::Properties(props) = prop
        {
            props.iter_mut().for_each(|p| {
                if p.has_name(prop_name) {
                    p.add_values(values);
                }
            });
        }
    }

    fn normalize_title(title: &str) -> String {
        title.replace("|", "-")
    }

    pub fn get_file_for_creator(&self, name: &str, create_missing: bool) -> Result<PathBuf> {
        let template_file = self
            .root_dir
            .join(".zk")
            .join("templates")
            .join("creator.md");
        self.get_zk_file(name, template_file, create_missing)
    }
}

impl TaskDataHandler for ZkHandler {
    #[instrument]
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool> {
        debug!("handling {task_data:?}");
        let Some(title) = task_data.get_title() else {
            debug!("no title!");
            return Ok(false);
        };
        let title = ZkHandler::normalize_title(&title);
        let template_file = match task_data {
            TaskData::Youtube(_url, _, _channel, _tags) => {
                self.root_dir.join(".zk/templates/yt_video.md")
            }
            TaskData::Sbs(_, _, _, _, _) => self.root_dir.join(".zk/templates/article.md"),
            TaskData::YtPlaylist(_, _, _) => self.root_dir.join(".zk/templates/yt_playlist.md"),
            TaskData::Interactive(template_name, _, _, _, _) => {
                self.root_dir.join(".zk/templates").join(template_name)
            }
            TaskData::Reddit(_, _, _) => self.root_dir.join(".zk/templates/article.md"),
            _ => todo!("not implemented: conversion of {task_data:?} to zk."),
        };
        debug!("using template {template_file:?}");
        let Ok(zk_file) = self.get_zk_file(&title, template_file, true) else {
            return Ok(false);
        };
        if !zk_file.exists() {
            println!("zk file {zk_file:?} was not created!");
            info!("zk file {zk_file:?} was not created!");
            return Ok(false);
        }
        debug!("parsing: {zk_file:?}");
        let pd = zk_parsing::parse_zk_file(&zk_file);
        debug!("{pd:?}");
        let mut pd = pd?;
        let success = self.add_to_zk_pd(&mut pd, task_data, &Some(zk_file.clone()));
        if success {
            let file_info = FileInfo::new_same_file(zk_file.clone());
            let text = pd.to_zk_text(&Some(file_info));
            debug!("added {task_data:?} to pd with result: {text:?}");

            std::fs::write(&zk_file, text).context(format!("Failed to write to {zk_file:?}!"))?;
            let mention =
                DocumentComponent::FileLink(MentionedFile::FilePath(zk_file), None, Some(title));
            let journal_mention = DocumentComponent::List(
                vec![ListElem::new(ParsedDocument::ParsedText(vec![mention]))],
                false,
            );
            let success = self.append_to_zk_journal(journal_mention)?;
            Ok(success)
        } else {
            debug!("failed to add {task_data:?}");
            Ok(false)
        }
    }

    fn get_template_names(&self) -> Result<Vec<String>> {
        let p = self.root_dir.join(".zk/templates");
        let dir_entries: Vec<DirEntry> = p
            .read_dir()?
            .map(|f| f.context(""))
            .collect::<Result<Vec<DirEntry>>>()?;
        let res: Result<Vec<Option<String>>> = dir_entries
            .into_iter()
            .map(|f| match f.file_type() {
                Ok(ft) => {
                    if ft.is_file() {
                        let name = f.file_name().into_string();
                        let tmp: Result<String> = match name {
                            std::result::Result::Ok(s) => anyhow::Ok(s),
                            std::result::Result::Err(s) => bail!("{s:?}"),
                        };
                        tmp.map(Some)
                    } else {
                        Ok(None)
                    }
                }
                _ => bail!("All direcory entries should have a file type"),
            })
            .collect();
        let res: Vec<String> = res?.into_iter().flatten().collect();
        Ok(res)
    }
}

#[ignore = "Test is hard to get right as the logic relies on the zk lookup file. A proper test would need some restructuring"]
#[test]
fn test_add_to_yt_pd() {
    use crate::parsing::zk_parsing::parse_zk_text;
    use crate::todoi::handlers::zk_handler::ZkHandler;
    // PROBLEM: this test currently relies on a bug introduced earlier: test_channel has file "" in
    // the lookup file.
    // Maybe this test should be disabled as it seems difficult to fix.
    // or we could provide the lookup table to add_to_zk_pd, which would make the code a bit more
    // complicated as then the caller would be responsible for managing the lookup table and
    // creating a new file if required.
    // Maybe it would be best to wrap the lookup table in a struct and to use a mock object for
    // tests
    let text = "---
date: 2024-12-31 01:09:55
tags: [video, youtube, inbox]
---

# title
- channel::= 
- description::= 
- url::= ";
    let res = parse_zk_text(text, &None);
    let Ok(mut pd) = res else {
        panic!("parsing failed: {res:?}");
    };
    let zk_handler = ZkHandler::new("/home/tobias/kasten".into());
    let task_data = TaskData::Youtube(
        "url".to_string(),
        "title".to_string(),
        "test_channel".to_string(),
        vec!["tag1".to_string(), "tag2".to_string()],
    );
    let _ = zk_handler.add_to_zk_pd(&mut pd, &task_data, &None);
    let res = pd.to_zk_text(&None);
    let expected = "---
date: 2024-12-31 01:09:55
tags: [video, youtube, inbox, tag1, tag2]
---

# title
- channel ::= [test_channel]()
- description ::= title
- url ::= url";
    assert_eq!(res, expected);
}
