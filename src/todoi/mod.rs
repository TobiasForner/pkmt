mod config;
mod interactive;
mod todoist_api;
mod youtube_details;
use std::{
    collections::HashMap,
    fmt::Debug,
    fs::DirEntry,
    path::{Path, PathBuf},
    str::FromStr,
    vec,
};
use test_log::test;

use anyhow::{bail, Context, Result};
use interactive::get_interactive_data;
use regex::Regex;
use tracing::{debug, info, instrument};

use crate::{
    document_component::{
        DocumentComponent, DocumentElement, FileInfo, MentionedFile, ParsedDocument, PropValue,
    },
    logseq_parsing::parse_logseq_file,
    parse::{parse_all_files_in_dir, parse_file, TextMode},
    todoi::{
        config::Config,
        interactive::Resolution,
        todoist_api::{TodoistAPI, TodoistTask},
        youtube_details::{youtube_details, youtube_playlist_details},
    },
    zk_parsing::{self},
};

#[derive(Debug)]
pub struct LogSeqTemplates {
    templates_pd: ParsedDocument,
}

impl LogSeqTemplates {
    pub fn new(logseq_graph_root: &Path) -> Result<Self> {
        let templates_file = logseq_graph_root
            .join("pages")
            .join("Templates.md")
            .canonicalize()
            .unwrap();

        let pd = parse_logseq_file(templates_file)?;
        Ok(Self { templates_pd: pd })
    }

    pub fn get_template_comp(&self, template_name: &str) -> Option<DocumentComponent> {
        use crate::document_component::DocumentElement::ListElement;
        self.templates_pd
            .get_document_component(&|c| match &c.element {
                ListElement(_, props) => props
                    .iter()
                    .any(|(key, value)| key == "template" && value == template_name),
                _ => false,
            })
    }

    pub fn template_names(&self) -> Vec<String> {
        use crate::document_component::DocumentElement::ListElement;
        let mut res = vec![];
        let template_comps = self.templates_pd.get_all_document_components(&|c| {
            if let ListElement(_, props) = &c.element {
                props.iter().any(|(key, _)| key == "template")
            } else {
                false
            }
        });
        template_comps.iter().for_each(|c| {
            if let ListElement(_, props) = &c.element {
                if let Some((_, value)) = props.iter().find(|(key, _)| key == "template") {
                    res.push(value.clone());
                }
            }
        });

        res
    }
}

/// gathers tasks and calls the correct handler
/// tasks are marked as completed if complete_tasks is set
pub fn main(root_dir: PathBuf, complete_tasks: bool, mode: TextMode) -> Result<()> {
    let config = Config::load(&PathBuf::from_str(
        "/mnt/c/Users/Tobias/AppData/Local/todoist_import/todoist_import/todoi_config.toml",
    )?)?;
    let todoist_api = TodoistAPI::new(&config.keys.todoist_api_key);
    let inbox = todoist_api.get_inbox()?;

    let mut inbox_tasks = todoist_api.get_project_tasks(&inbox)?;
    inbox_tasks.sort_by_key(|t| t.content.clone());
    info!("Retrieved todoist tasks.");
    inbox_tasks.dedup_by_key(|t| t.content.clone());
    debug!("mode: {mode:?}");

    let completed_tasks = match mode {
        TextMode::Zk => add_to_zk(root_dir, &inbox_tasks, &config),
        TextMode::LogSeq => add_to_logseq(root_dir, &inbox_tasks, &config),
        TextMode::Obsidian => todo!("not implemented!"),
    }?;

    if complete_tasks {
        completed_tasks.iter().for_each(|t| {
            println!("Completing: {}", t.content);
            todoist_api.close_task(t);
        });
    }
    Ok(())
}

#[instrument(skip_all)]
fn add_to_zk(
    zk_root_dir: PathBuf,
    inbox_tasks: &[TodoistTask],
    config: &Config,
) -> Result<Vec<TodoistTask>> {
    let mut zk_handler = ZkHandler::new(zk_root_dir);
    let all_urls = zk_handler.get_all_urls()?;
    let deduped_tasks: Vec<TodoistTask> = inbox_tasks
        .iter()
        .filter_map(|t| {
            if all_urls.iter().any(|u| t.content.contains(u)) {
                None
            } else {
                Some(t.clone())
            }
        })
        .collect();
    let res = handle_tasks(&deduped_tasks, &mut zk_handler, config);
    debug!("handled tasks");
    res
}

pub fn set_zk_creator_file(name: &str, new_file: &PathBuf) -> Result<()> {
    if !new_file.exists() {
        bail!("new creator file {new_file:?} does not exist!");
    }
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let data_dir = base_dirs.data_dir().join("pkmt");
        if !data_dir.exists() {
            std::fs::create_dir(&data_dir).context("Could not create {data_dir:?}")?;
        }

        let lookup_path = data_dir.join("creator_lookup.toml");
        let mut lookup: HashMap<String, PathBuf> = if lookup_path.exists() {
            debug!("loading lookup table from file.");
            let text = std::fs::read_to_string(&lookup_path)
                .context("Expected {lookup_path:?} to exist!")?;
            toml::from_str(&text)?
        } else {
            debug!("creating now lookup table.");
            HashMap::new()
        };
        lookup.insert(name.to_string(), new_file.clone());
        let text = toml::to_string(&lookup)?;
        std::fs::write(&lookup_path, text)
            .context(format!("Could not write to {lookup_path:?}"))?;
        Ok(())
    } else {
        bail!("Could not create basedirs!")
    }
}

pub fn get_zk_creator_file(root_dir: &Path, name: &str) -> Result<PathBuf> {
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let data_dir = base_dirs.data_dir().join("pkmt");
        if !data_dir.exists() {
            std::fs::create_dir(&data_dir).context("Could not create {data_dir:?}")?;
        }

        let lookup_path = data_dir.join("creator_lookup.toml");
        let mut lookup: HashMap<String, PathBuf> = if lookup_path.exists() {
            debug!("loading lookup table from file.");
            let text = std::fs::read_to_string(&lookup_path)
                .context("Expected {lookup_path:?} to exist!")?;
            toml::from_str(&text)?
        } else {
            debug!("creating now lookup table.");
            HashMap::new()
        };
        if let Some(path) = lookup.get(name) {
            debug!("{name:?}: found creator file in lookup: {path:?}");
            Ok(path.to_path_buf())
        } else {
            let template_file = root_dir.join(".zk").join("templates").join("creator.md");
            let file = ZkHandler::get_zk_file(name, template_file)?;
            debug!("{name:?}: created new creator file: {file:?}");
            lookup.insert(name.to_string(), file.clone());
            let text = toml::to_string(&lookup)?;
            std::fs::write(&lookup_path, text)
                .context(format!("Could not write to {lookup_path:?}"))?;
            Ok(file)
        }
    } else {
        bail!("Could not create basedirs!")
    }
}

#[derive(Debug)]
pub struct ZkHandler {
    root_dir: PathBuf,
}

impl ZkHandler {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    #[instrument]
    fn get_zk_file(title: &str, template_path: PathBuf) -> Result<PathBuf> {
        use std::process::Command;
        debug!("trying to get zk file for {title}");
        let output = Command::new("zk")
            .arg("new")
            .arg("--no-input")
            .arg(format!("--title={title}"))
            .arg(format!("--template={}", template_path.to_str().unwrap()))
            .arg("-p")
            .output()
            .context(format!("failed to retrieve zk file for {title}"))?;
        if !output.status.success() {
            println!("Failed to create zk file for title {title:?}!");
            bail!("Could not create zk file for {title:?}");
        }
        let p = std::str::from_utf8(&output.stdout)?;
        Ok(PathBuf::from_str(p.trim())?)
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
        let file = get_zk_creator_file(&self.root_dir, author)?;
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
        let frontmatter = pd.get_document_component_mut(&|dc| {
            let elem = dc.get_element();
            matches!(elem, DocumentElement::Frontmatter(_))
        });

        let tags_to_add: Vec<String> = task_data
            .get_tags()
            .iter()
            .map(|t| t.trim_start_matches('#').to_string())
            .collect();

        let tags_success = if let Some(dc) = frontmatter {
            if let DocumentElement::Frontmatter(properties) = dc.get_element_mut() {
                for p in properties {
                    if p.has_name("tags") {
                        p.add_values_parse(&tags_to_add, &TextMode::Zk, file_dir);
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if tags_success {
            match task_data {
                TaskData::Sbs(url, author, _, _) => {
                    self.fill_property(pd, "url", &[url.to_string()], file_dir);
                    if let Some(author) = author {
                        let success = self.fill_in_creator(pd, author, "source", file_dir);
                        if success.is_err() {
                            return false;
                        }
                        let success = self.fill_in_creator(pd, "sbs", "source", file_dir);
                        if success.is_err() {
                            return false;
                        }
                        //self.fill_property(pd, "author", &[author.to_string()], file_dir);
                    }
                }
                TaskData::Youtube(url, title, channel, _) => {
                    self.fill_property(pd, "url", &[url.to_string()], file_dir);
                    //self.fill_property(pd, "channel", &[channel.to_string()], file_dir);
                    let success = self.fill_in_creator(pd, channel, "channel", file_dir);
                    if success.is_err() {
                        println!("Could not fill in creator for {url:?}: {success:?}");
                        return false;
                    }
                    self.fill_property(pd, "description", &[title.to_string()], file_dir);
                }
                TaskData::YtPlaylist(url, channel, _) => {
                    self.fill_property(pd, "url", &[url.to_string()], file_dir);
                    //self.fill_property(pd, "channel", &[channel.to_string()], file_dir);
                    let success = self.fill_in_creator(pd, channel, "channel", file_dir);
                    if success.is_err() {
                        return false;
                    }
                }
                TaskData::Unhandled => {
                    return false;
                }
                TaskData::Interactive(_, url, _, _) => {
                    if let Some(url) = url {
                        debug!("filled in url");
                        self.fill_property(pd, "url", &[url.to_string()], file_dir);
                    }
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
        let file_info =
            FileInfo::try_new(journal_path.clone(), Some(journal_path.clone()), None, None)?;
        let journal_text = pd.to_zk_text(&Some(file_info));
        debug!("new journal text: {journal_text:?}");

        std::fs::write(&journal_path, journal_text)
            .context(format!("Could not write file {journal_path:?}"))?;
        Ok(true)
    }

    fn get_all_urls(&self) -> Result<Vec<String>> {
        let parsed_documents = parse_all_files_in_dir(&self.root_dir, &TextMode::Zk)?;
        let prop_dcs: Vec<DocumentComponent> = parsed_documents
            .iter()
            .flat_map(|pd| {
                pd.get_all_document_components(&|dc: &DocumentComponent| {
                    if let DocumentElement::Properties(props) = dc.get_element() {
                        props.iter().any(|p| p.has_name("url"))
                    } else {
                        false
                    }
                })
                .into_iter()
            })
            .collect();
        let tmp: Vec<String> = prop_dcs
            .iter()
            .filter_map(|dc| {
                if let DocumentElement::Properties(props) = dc.get_element() {
                    let tmp = props.iter().filter(|p| p.has_name("url")).flat_map(|p| {
                        p.values.iter().filter_map(|v| match v {
                            PropValue::String(s) => Some(s.clone()),
                            _ => None,
                        })
                    });
                    Some(tmp)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        Ok(tmp)
    }

    #[instrument]
    fn fill_property(
        &self,
        pd: &mut ParsedDocument,
        prop_name: &str,
        values: &[String],
        file_dir: &Option<PathBuf>,
    ) {
        let property = pd.get_document_component_mut(&|dc| match dc.get_element() {
            DocumentElement::Properties(props) => props.iter().any(|p| p.has_name(prop_name)),
            _ => false,
        });
        if let Some(prop) = property {
            if let DocumentElement::Properties(props) = prop.get_element_mut() {
                props.iter_mut().for_each(|p| {
                    if p.has_name(prop_name) {
                        p.add_values_parse(values, &TextMode::Zk, file_dir);
                    }
                });
            }
        }
    }

    #[instrument]
    fn fill_props(
        &self,
        pd: &mut ParsedDocument,
        prop_name: &str,
        values: &[PropValue],
        file_dir: &Option<PathBuf>,
    ) {
        let property = pd.get_document_component_mut(&|dc| match dc.get_element() {
            DocumentElement::Properties(props) => props.iter().any(|p| p.has_name(prop_name)),
            _ => false,
        });
        if let Some(prop) = property {
            if let DocumentElement::Properties(props) = prop.get_element_mut() {
                props.iter_mut().for_each(|p| {
                    if p.has_name(prop_name) {
                        p.add_values(values);
                    }
                });
            }
        }
    }
}

impl TaskDataHandler for ZkHandler {
    #[instrument]
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool> {
        debug!("handling {task_data:?}");
        if let Some(url) = task_data.get_url() {
            if url_is_duplicate(url, &self.root_dir, &TextMode::Zk)? {
                info!("Duplicate url: {url}! Skipping {task_data:?}");
                return Ok(false);
            }
        }
        let Some(title) = task_data.get_title() else {
            debug!("no title!");
            return Ok(false);
        };
        let template_file = match task_data {
            TaskData::Youtube(_url, _, _channel, _tags) => {
                self.root_dir.join(".zk/templates/yt_video.md")
            }
            TaskData::Sbs(_, _, _, _) => self.root_dir.join(".zk/templates/article.md"),
            TaskData::YtPlaylist(_, _, _) => self.root_dir.join(".zk/templates/yt_playlist.md"),
            TaskData::Interactive(template_name, _, _, _) => {
                self.root_dir.join(".zk/templates").join(template_name)
            }
            _ => todo!("not implemented: conversion of {task_data:?} to zk."),
        };
        debug!("using template {template_file:?}");
        let Ok(zk_file) = ZkHandler::get_zk_file(&title, template_file) else {
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
            let file_info = FileInfo::try_new(zk_file.clone(), Some(zk_file.clone()), None, None)?;
            let text = pd.to_zk_text(&Some(file_info));
            debug!("added {task_data:?} to pd with result: {text:?}");

            std::fs::write(&zk_file, text).context(format!("Failed to write to {zk_file:?}!"))?;
            let mention = DocumentComponent::new(DocumentElement::FileLink(
                MentionedFile::FilePath(zk_file),
                None,
                Some(title),
            ));
            let journal_mention = DocumentComponent::new(DocumentElement::ListElement(
                ParsedDocument::ParsedText(vec![mention]),
                vec![],
            ));
            let success = self.append_to_zk_journal(journal_mention)?;
            Ok(success)
        } else {
            debug!("failed to add {task_data:?}");
            Ok(false)
        }
    }

    fn get_template_names(&self) -> Result<Vec<String>> {
        // TODO: remove unwraps
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
                        //let tmp: Result<String> = f.file_name().into_string();
                        //Ok(Some(name))
                    } else {
                        Ok(None)
                    }
                }
                _ => bail!(""),
            })
            .collect();
        let res: Vec<String> = res?.into_iter().flatten().collect();
        Ok(res)
    }
}

fn add_to_logseq(
    logseq_graph_root: PathBuf,
    inbox_tasks: &[TodoistTask],
    config: &Config,
) -> Result<Vec<TodoistTask>> {
    /*let today = chrono::offset::Local::now();
        let todays_journal_file = logseq_graph_root
            .join("journals")
            .join(today.format("%Y_%m_%d.md").to_string());
        let todays_journal = if todays_journal_file.exists() {
            println!("loaded existing journal file");
            parse_logseq_file(&todays_journal_file)?
        } else {
            println!("creating new journal file!");
            ParsedDocument::ParsedFile(vec![], todays_journal_file.clone())
        };

        // remove empty list elements from the end of the journal
        let mut end = true;
        let filtered_components = todays_journal
            .components()
            .iter()
            .rev()
            .filter(|c| {
                if !c.is_empty_list() {
                    end = false;
                    true
                } else {
                    !end
                }
            })
            .cloned()
            .rev()
            .collect();
        let mut todays_journal = todays_journal.with_components(filtered_components);
        let templates = LogSeqTemplates::new(&logseq_graph_root)?;
    */
    /*
    let mut completed_tasks =
        handle_youtube_tasks(&inbox_tasks, &templates, &mut todays_journal, &config);

    let remaining_tasks: Vec<TodoistTask> = inbox_tasks
        .into_iter()
        .filter(|task| !completed_tasks.contains(task))
        .collect();

    let mut new_completions = handle_sbs_tasks(&remaining_tasks, &templates, &mut todays_journal);
    completed_tasks.append(&mut new_completions);
    let remaining_tasks: Vec<TodoistTask> = remaining_tasks
        .into_iter()
        .filter(|task| !completed_tasks.contains(task))
        .collect();

    let mut new_completions =
        handle_youtube_playlists(&remaining_tasks, &templates, &mut todays_journal, &config);
    completed_tasks.append(&mut new_completions);
    let remaining_tasks: Vec<TodoistTask> = remaining_tasks
        .into_iter()
        .filter(|task| !new_completions.contains(task))
        .collect();

    let mut cancelled = false;
    remaining_tasks.iter().for_each(|t| {
        if !cancelled {
            let res = handle_interactive(t, &mut todays_journal, &templates, &config);
            println!("{t:?}: {res:?}");
            match res {
                Resolution::Cancel => {
                    cancelled = true;
                }
                Resolution::Skip => {}
                Resolution::Complete => {
                    completed_tasks.push(t.clone());
                }
            }
        }
    });*/
    let mut logseq_handler = LogSeqHandler::new(logseq_graph_root)?;
    handle_tasks(inbox_tasks, &mut logseq_handler, config)
}

fn get_task_data_non_interactive(
    tasks: &[TodoistTask],
    config: &Config,
) -> Vec<(TaskData, TodoistTask)> {
    let tasks = tasks.iter().map(|t| (handle_youtube_task(t, config), t));
    let tasks = tasks.map(|(td, task)| match td {
        TaskData::Unhandled => (handle_sbs_task(task), task),
        _ => (td, task),
    });
    let tasks = tasks.map(|(td, task)| match td {
        TaskData::Unhandled => (handle_youtube_playlist(task, config), task),
        _ => (td, task),
    });
    tasks.map(|(td, task)| (td, task.clone())).collect()
}

trait TaskDataHandler {
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool>;
    fn get_template_names(&self) -> Result<Vec<String>>;
}
#[derive(Debug)]
struct LogSeqHandler {
    templates: LogSeqTemplates,
    todays_journal: ParsedDocument,
    todays_journal_file: PathBuf,
}

impl LogSeqHandler {
    fn new(graph_root: PathBuf) -> Result<Self> {
        let today = chrono::offset::Local::now();
        let todays_journal_file = graph_root
            .join("journals")
            .join(today.format("%Y_%m_%d.md").to_string());
        let todays_journal = if todays_journal_file.exists() {
            println!("loaded existing journal file");
            parse_logseq_file(&todays_journal_file)?
        } else {
            println!("creating new journal file!");
            ParsedDocument::ParsedFile(vec![], todays_journal_file.clone())
        };

        // remove empty list elements from the end of the journal
        let mut end = true;
        let filtered_components = todays_journal
            .components()
            .iter()
            .rev()
            .filter(|c| {
                if !c.is_empty_list() {
                    end = false;
                    true
                } else {
                    !end
                }
            })
            .cloned()
            .rev()
            .collect();
        let todays_journal = todays_journal.with_components(filtered_components);
        let templates = LogSeqTemplates::new(&graph_root)?;
        let res = LogSeqHandler {
            templates,
            todays_journal,
            todays_journal_file,
        };
        Ok(res)
    }
}

impl TaskDataHandler for LogSeqHandler {
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool> {
        use crate::document_component::DocumentElement::ListElement;
        use TaskData::*;
        match task_data {
            Youtube(url, title, channel, tags) => {
                let mut yt_template = self
                    .templates
                    .get_template_comp("youtube")
                    .expect("No youtube template!")
                    .clone();
                if let ListElement(_, props) = yt_template.get_element_mut() {
                    let mut add = vec![];
                    add.push(("authors", vec![format!("[[{}]]", channel)]));
                    add.push(("description", vec![title.clone()]));
                    add.push(("tags", tags.clone()));
                    *props = fill_properties(props, &add, &["template"]);

                    // add embed
                    let embed_block = yt_template
                        .get_nth_child_mut(0)
                        .unwrap()
                        .get_nth_child_mut(0)
                        .unwrap();

                    let embed = if url.contains("/shorts/") {
                        DocumentComponent::new_text(url)
                    } else {
                        DocumentComponent::new_text(&format!("{{{{video {url}}}}}"))
                    };
                    let pd = ParsedDocument::ParsedText(vec![embed]);
                    let elem = ListElement(pd, vec![]);
                    embed_block.element = elem;
                }
                self.todays_journal.add_component(yt_template);
            }
            TaskData::Sbs(url, author, title, tags) => {
                if let Some(comp) = self.templates.get_template_comp("article") {
                    let mut comp = comp.clone();
                    if let ListElement(_, props) = comp.get_element_mut() {
                        let mut source = vec!["[[Stronger by Science]]".to_string()];
                        if let Some(author) = author {
                            source.push(author.clone());
                        }
                        let url = vec![url.clone()];
                        let mut add: Vec<(&str, Vec<String>)> =
                            vec![("source", source), ("url", url), ("tags", tags.clone())];
                        if let Some(title) = title {
                            add.push(("description", vec![title.clone()]));
                        }
                        *props = fill_properties(props, &add, &["template"]);
                        self.todays_journal.add_component(comp);
                    }
                }
            }
            TaskData::YtPlaylist(url, channel, title) => {
                let mut temp = self
                    .templates
                    .get_template_comp("youtube_playlist")
                    .unwrap();
                if let ListElement(_, props) = temp.get_element_mut() {
                    *props = fill_properties(
                        props,
                        &[
                            ("description", vec![title.to_string()]),
                            ("authors", vec![format!("[[{channel}]]")]),
                            ("url", vec![url.to_string()]),
                        ],
                        &["template"],
                    );
                    self.todays_journal.add_component(temp);
                }
            }
            TaskData::Interactive(template_name, url, title, tags) => {
                let mut comp = self.templates.get_template_comp(template_name).unwrap();
                if let ListElement(_, props) = comp.get_element_mut() {
                    let mut add = vec![];
                    if let Some(title) = title {
                        add.push(("description", vec![title.clone()]));
                    }
                    add.push(("tags", tags.clone()));
                    if let Some(url) = url {
                        add.push(("url", vec![url.clone()]))
                    }
                    let new_props = fill_properties(props, &add, &["template"]);
                    *props = new_props;
                    self.todays_journal.add_component(comp);
                }
            }
            _ => {
                return Ok(false);
            }
        }

        std::fs::write(
            &self.todays_journal_file,
            self.todays_journal.to_logseq_text(&None),
        )
        .context(format!("Could not write to {:?}", self.todays_journal_file))?;
        Ok(true)
    }
    fn get_template_names(&self) -> Result<Vec<String>> {
        Ok(self.templates.template_names())
    }
}

fn get_task_data_full(
    tasks: &[TodoistTask],
    config: &Config,
    template_names: &[String],
) -> Vec<(TaskData, TodoistTask)> {
    let tasks = get_task_data_non_interactive(tasks, config);
    // handle interactive
    let mut cancelled = false;
    tasks
        .into_iter()
        .map(|(td, task)| match td {
            TaskData::Unhandled => {
                if !cancelled {
                    let (res, td) = get_interactive_data(&task, template_names, config);
                    println!("interactive resolution for {task:?}: {res:?} with {td:?}");
                    if let Resolution::Cancel = res {
                        cancelled = true;
                    }
                    (td, task)
                } else {
                    (td, task)
                }
            }
            _ => (td, task),
        })
        .collect()
}

#[instrument(skip_all)]
fn handle_tasks<T>(
    tasks: &[TodoistTask],
    handler: &mut T,
    config: &Config,
) -> Result<Vec<TodoistTask>>
where
    T: TaskDataHandler + Debug,
{
    debug!("handler: {handler:?}");
    let tasks = get_task_data_full(tasks, config, &handler.get_template_names()?);
    let tasks: Result<Vec<(bool, TodoistTask)>> = tasks
        .into_iter()
        .map(|(td, task)| handler.handle_task_data(&td).map(|e| (e, task)))
        .collect();
    debug!("filtering handled tasks: {tasks:?}");
    let tasks = tasks?
        .iter()
        .filter_map(|(done, task)| if *done { Some(task.clone()) } else { None })
        .collect();
    Ok(tasks)
}

#[derive(Debug)]
pub enum TaskData {
    Unhandled,
    /// url, title, channel, tags
    Youtube(String, String, String, Vec<String>),
    /// url, optional author, optional title, tags
    Sbs(String, Option<String>, Option<String>, Vec<String>),
    /// url, channel, title
    YtPlaylist(String, String, String),
    /// template_name, optional url, optional title, tags
    Interactive(String, Option<String>, Option<String>, Vec<String>),
}

impl TaskData {
    fn get_title(&self) -> Option<String> {
        use TaskData::*;
        match self {
            Youtube(_, title, _, _) => Some(title.to_string()),
            Sbs(_, _, title, _) => title.clone(),
            YtPlaylist(_, _, title) => Some(title.to_string()),
            Interactive(_, _, title, _) => title.clone(),
            _ => None,
        }
    }
    fn get_tags(&self) -> Vec<String> {
        use TaskData::*;
        match self {
            Unhandled => vec![],
            Youtube(_, _, _, tags) => tags.clone(),
            Sbs(_, _, _, tags) => tags.clone(),
            YtPlaylist(_, _, _) => vec![],
            Interactive(_, _, _, tags) => tags.clone(),
        }
    }

    fn get_url(&self) -> Option<&str> {
        use TaskData::*;
        match self {
            Unhandled => None,
            Youtube(url, _, _, _) => Some(url),
            Sbs(url, _, _, _) => Some(url),
            YtPlaylist(url, _, _) => Some(url),
            Interactive(_, url, _, _) => url.as_deref(),
        }
    }
}

fn handle_youtube_task(task: &TodoistTask, config: &Config) -> TaskData {
    let yt_video_url_re =
        Regex::new(r"(https://)(?:www\.)?(?:youtu.be|youtube\.com)/(shorts/)?[A-Za-z0-9?=\-_&]*")
            .unwrap();
    if let Some(m) = yt_video_url_re.captures(&task.content) {
        if let Some(video_url) = m.get(0) {
            let video_url = video_url.as_str();
            if let Ok((video_title, authors)) = youtube_details(video_url, &config.keys.yt_api_key)
            {
                let mut tags = vec![];

                if let Some(mut ct) = config.get_channel_tags(&authors) {
                    tags.append(&mut ct);
                }

                tags.append(&mut config.get_keyword_tags(&video_title));
                tags.sort();
                tags.dedup();
                return TaskData::Youtube(video_url.into(), video_title, authors, tags);
            }
        }
    }
    TaskData::Unhandled
}

#[instrument]
fn handle_sbs_task(task: &TodoistTask) -> TaskData {
    let sbs_link_re =
        Regex::new(r"https://ckarchive\.com/b/[a-zA-Z0-9]*\?ck_subscriber_id=2334581400").unwrap();
    let author_re = Regex::new(r" newsletter is by ([a-zA-Z\.\s]*).&lt;/h3&gt;").unwrap();

    if let Some(art_url) = sbs_link_re.captures(&task.content) {
        if let Some(art_url) = art_url.get(0) {
            let article_url = art_url.as_str();
            debug!("found sbs url {article_url}");
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let res = runtime.block_on(reqwest::get(article_url)).unwrap();
            let text = runtime.block_on(res.text()).unwrap();
            let author = if let Some(author) = author_re.captures(&text) {
                let mut author = author.get(1).unwrap().as_str().to_string();
                if author.ends_with('.') {
                    author.remove(author.len() - 1);
                }
                Some(author)
            } else {
                None
            };
            let title =
                if let (Some(start), Some(end)) = (text.find("<title>"), text.find("</title>")) {
                    Some(text[start + 7..end].to_string())
                } else {
                    None
                };
            let tags = vec!["fitness".to_string()];
            let res = TaskData::Sbs(article_url.to_string(), author, title, tags);
            debug!("found {res:?} for {task:?}");
            return res;
        }
    }

    let sbs_website_re = Regex::new(r"https://www.strongerbyscience.com/[a-zA-Z-]+/").unwrap();
    let author_re = Regex::new("<meta name=\"author\" content=\"([a-zA-Z\\s\\-]+)\" />").unwrap();
    if let Some(art_url) = sbs_website_re.captures(&task.content) {
        if let Some(art_url) = art_url.get(0) {
            let article_url = art_url.as_str();
            debug!("found sbs website url {article_url}");
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let res = runtime.block_on(reqwest::get(article_url)).unwrap();
            let text = runtime.block_on(res.text()).unwrap();

            //println!("pos: {pos:?}; {text:?}");
            let author = if let Some(author) = author_re.captures(&text) {
                let mut author = author.get(1).unwrap().as_str().to_string();
                if author.ends_with('.') {
                    author.remove(author.len() - 1);
                }
                Some(author)
            } else {
                None
            };
            let title = if let (Some(start), Some(end)) =
                (text.find("<title>"), text.find("</title>"))
            {
                let title = text[start + 7..end].trim_end_matches(" &#8226; Stronger by Science");
                Some(title.to_string())
            } else {
                None
            };
            let tags = vec!["fitness".to_string()];
            let res = TaskData::Sbs(article_url.to_string(), author, title, tags);
            debug!("found {res:?} for {task:?}");
            return res;
        }
    }
    TaskData::Unhandled
}

fn handle_youtube_playlist(task: &TodoistTask, config: &Config) -> TaskData {
    let playlist_re = Regex::new(r"https://www\.youtube\.com/playlist\?list=[a-zA-Z0-9]+").unwrap();
    if playlist_re.captures(&task.content).is_some() {
        let playlist_url = task.content.clone();
        if let Ok((description, channel)) =
            youtube_playlist_details(&playlist_url, &config.keys.yt_api_key)
        {
            return TaskData::YtPlaylist(playlist_url, channel, description);
        }
    }
    TaskData::Unhandled
}

fn handle_youtube_tasks(
    tasks: &[TodoistTask],
    templates: &LogSeqTemplates,
    journal_file: &mut ParsedDocument,
    config: &Config,
) -> Vec<TodoistTask> {
    use crate::document_component::DocumentElement::ListElement;
    let yt_video_url_re =
        Regex::new(r"(https://)(?:www\.)?(?:youtu.be|youtube\.com)/(shorts/)?[A-Za-z0-9?=\-_]*")
            .unwrap();

    let yt_template = templates
        .get_template_comp("youtube")
        .expect("No youtube template!");

    tasks
        .iter()
        .filter_map(|task| yt_video_url_re.captures(&task.content).map(|m| (task, m)))
        .filter_map(|(task, c)| c.get(0).map(|m| (task, m.as_str())))
        .filter(|(_, video_url)| !video_url.contains("/playlist"))
        .map(|(task, video_url)| {
            let mut yt_template = yt_template.clone();

            // remove template tag and add new tags
            if let ListElement(_, props) = yt_template.get_element_mut() {
                let mut add = vec![];
                let mut tags = vec![];

                if let Ok((video_title, authors)) =
                    youtube_details(video_url, &config.keys.yt_api_key)
                {
                    add.push(("authors", vec![format!("[[{authors}]]")]));
                    if let Some(mut ct) = config.get_channel_tags(&authors) {
                        tags.append(&mut ct);
                    }

                    tags.append(&mut config.get_keyword_tags(&video_title));
                    tags.sort();
                    tags.dedup();
                    add.push(("description", vec![video_title]));
                }

                add.push(("tags", tags));
                *props = fill_properties(props, &add, &["template"]);

                // add embed
                let embed_block = yt_template
                    .get_nth_child_mut(0)
                    .unwrap()
                    .get_nth_child_mut(0)
                    .unwrap();

                let embed = if video_url.contains("/shorts/") {
                    DocumentComponent::new_text(video_url)
                } else {
                    DocumentComponent::new_text(&format!("{{{{video {video_url}}}}}"))
                };
                let pd = ParsedDocument::ParsedText(vec![embed]);
                let elem = ListElement(pd, vec![]);
                embed_block.element = elem;
            }

            journal_file.add_component(yt_template);
            task
        })
        .cloned()
        .collect()
}

// returns completed tasks
fn handle_youtube_playlists(
    tasks: &[TodoistTask],
    templates: &LogSeqTemplates,
    journal_file: &mut ParsedDocument,
    config: &Config,
) -> Vec<TodoistTask> {
    use crate::document_component::DocumentElement::ListElement;
    let playlist_re = Regex::new(r"https://www\.youtube\.com/playlist\?list=[a-zA-Z0-9]+").unwrap();
    tasks
        .iter()
        .filter_map(|task| playlist_re.captures(&task.content).map(|m| (task, m)))
        .filter_map(|(task, c)| c.get(0).map(|m| (task, m.as_str())))
        .filter_map(|(task, playlist_url)| {
            let mut temp = templates.get_template_comp("youtube_playlist").unwrap();
            if let ListElement(_, props) = temp.get_element_mut() {
                if let Ok((description, channel)) =
                    youtube_playlist_details(playlist_url, &config.keys.yt_api_key)
                {
                    *props = fill_properties(
                        props,
                        &[
                            ("description", vec![description]),
                            ("authors", vec![format!("[[{channel}]]")]),
                            ("url", vec![playlist_url.to_string()]),
                        ],
                        &["template"],
                    );
                    journal_file.add_component(temp);
                    Some(task)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .cloned()
        .collect()
}

pub fn fill_properties(
    props: &[(String, String)],
    add: &[(&str, Vec<String>)],
    drop: &[&str],
) -> Vec<(String, String)> {
    let mut res = vec![];
    props.iter().for_each(|(key, val)| {
        let mut vals = vec![];
        if let Some((_, to_add)) = add.iter().find(|(k, _)| k == key) {
            if !val.trim().is_empty() {
                vals.push(val.to_string());
            }
            vals.extend(to_add.iter().map(|s| s.to_string()));
        }
        if !drop.contains(&key.as_str()) {
            if vals.is_empty() {
                vals.push(val.to_string());
            }
            res.push((key.to_string(), vals.join(", ")))
        }
    });
    res
}

fn handle_sbs_tasks(
    tasks: &[TodoistTask],
    templates: &LogSeqTemplates,
    journal_file: &mut ParsedDocument,
) -> Vec<TodoistTask> {
    use crate::document_component::DocumentElement::ListElement;
    let sbs_link_re =
        Regex::new(r"https://ckarchive\.com/b/[a-zA-Z0-9]*\?ck_subscriber_id=2334581400").unwrap();
    let author_re = Regex::new(r" newsletter is by ([a-zA-Z\.\s]*).&lt;/h3&gt;").unwrap();

    if let Some(comp) = templates.get_template_comp("article") {
        tasks
            .iter()
            .filter_map(|t| sbs_link_re.captures(&t.content).map(|c| (t, c)))
            .filter_map(|(t, c)| c.get(0).map(|m| (t, m)))
            .filter_map(|(task, text)| {
                let mut comp = comp.clone();
                if let ListElement(_, props) = comp.get_element_mut() {
                    let article_url = text.as_str();

                    let mut source = vec!["[[Stronger by Science]]".to_string()];
                    let runtime = tokio::runtime::Runtime::new().unwrap();
                    let res = runtime.block_on(reqwest::get(article_url)).unwrap();
                    let text = runtime.block_on(res.text()).unwrap();
                    if let Some(author) = author_re.captures(&text) {
                        let mut author = author.get(1).unwrap().as_str().to_string();
                        if author.ends_with('.') {
                            author.remove(author.len() - 1);
                        }
                        source.push(format!("[[{author}]]"));
                    }

                    let url = vec![article_url.to_string()];
                    let mut add: Vec<(&str, Vec<String>)> = vec![
                        ("source", source),
                        ("url", url),
                        ("tags", vec!["#Fitness".to_string()]),
                    ];
                    if let (Some(start), Some(end)) = (text.find("<title>"), text.find("</title>"))
                    {
                        let title = vec![text[start + 7..end].to_string()];
                        add.push(("description", title));
                    }
                    *props = fill_properties(props, &add, &["template"]);
                    journal_file.add_component(comp);
                    Some(task.clone())
                } else {
                    None
                }
            })
            .collect()
    } else {
        vec![]
    }
}

fn url_is_duplicate(url: &str, root_dir: &PathBuf, mode: &TextMode) -> Result<bool> {
    let parsed_documents = parse_all_files_in_dir(root_dir, mode)?;
    let mut res = false;
    parsed_documents.iter().for_each(|pd| {
        if pd
            .get_document_component(&|dc: &DocumentComponent| {
                if let DocumentElement::Properties(props) = dc.get_element() {
                    props.iter().any(|p| {
                        p.has_name("url") && p.has_value(&PropValue::String(url.to_string()))
                    })
                } else {
                    false
                }
            })
            .is_some()
        {
            res = true;
        }
    });
    Ok(res)
}

#[test]
fn test_add_to_yt_pd() {
    use zk_parsing::parse_zk_text;
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
