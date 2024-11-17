mod config;
mod interactive;
mod todoist_api;
mod youtube_details;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Result;
use clap::Command;
use interactive::handle_interactive_data;
use regex::Regex;

use crate::{
    document_component::{DocumentComponent, ParsedDocument},
    logseq_parsing::parse_logseq_file,
    todoi::{
        config::Config,
        interactive::{handle_interactive, Resolution},
        todoist_api::{TodoistAPI, TodoistTask},
        youtube_details::{youtube_details, youtube_playlist_details},
    },
    zk_parsing,
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
pub fn main(root_dir: PathBuf, complete_tasks: bool) -> Result<()> {
    let config = Config::parse(PathBuf::from_str(
        "/mnt/c/Users/Tobias/AppData/Local/todoist_import/todoist_import/todoi_config.toml",
    )?);
    let todoist_api = TodoistAPI::new(&config.todoist_api_key);
    let inbox = todoist_api.get_inbox()?;

    let inbox_tasks = todoist_api.get_project_tasks(&inbox)?;
    println!("Retrieved todoist tasks.");
    let completed_tasks = add_to_logseq(root_dir, &inbox_tasks, &config)?;

    if complete_tasks {
        completed_tasks.iter().for_each(|t| {
            println!("Completing: {}", t.content);
            todoist_api.close_task(t);
        });
    }
    Ok(())
}

fn add_to_logseq(
    logseq_graph_root: PathBuf,
    inbox_tasks: &[TodoistTask],
    config: &Config,
) -> Result<Vec<TodoistTask>> {
    let today = chrono::offset::Local::now();
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
    let completed_tasks = handle_tasks(inbox_tasks, &templates, &mut todays_journal, config);
    std::fs::write(todays_journal_file, todays_journal.to_logseq_text(&None))?;
    Ok(completed_tasks)
}

fn handle_tasks(
    tasks: &[TodoistTask],
    templates: &LogSeqTemplates,
    journal_file: &mut ParsedDocument,
    config: &Config,
) -> Vec<TodoistTask> {
    let tasks = tasks.iter().map(|t| (handle_youtube_task(t, config), t));
    let tasks = tasks.map(|(td, task)| match td {
        TaskData::Unhandled => (handle_sbs_task(task), task),
        _ => (td, task),
    });
    let tasks = tasks.map(|(td, task)| match td {
        TaskData::Unhandled => (handle_youtube_playlist(task, config), task),
        _ => (td, task),
    });

    // handle interactive
    let mut cancelled = false;
    let tasks = tasks.map(|(td, task)| match td {
        TaskData::Unhandled => {
            if !cancelled {
                let (res, td) = handle_interactive_data(task, templates, config);
                println!("{task:?}: {res:?}");
                match res {
                    Resolution::Cancel => {
                        cancelled = true;
                    }
                    Resolution::Skip => {}
                    Resolution::Complete => {}
                }
                (td, task)
            } else {
                (td, task)
            }
        }
        _ => (td, task),
    });

    tasks
        .filter(|(td, _)| td.add_to_logseq_journal(templates, journal_file))
        .map(|(_, task)| task.clone())
        .collect()
}

pub fn test_zk(root_dir: PathBuf) {
    let title = "test_title";
    let template_file = root_dir.join(".zk/templates/yt_video.md");
    use std::process::Command;
    let output = Command::new("zk")
        .arg("new")
        .arg("--no-input")
        .arg(format!("--title={title}"))
        .arg(format!("--template={}", template_file.to_str().unwrap()))
        .arg("-p")
        .output();
    println!("Got {output:?}");
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
    fn add_to_zk(&self, root_dir: PathBuf) -> bool {
        match self {
            TaskData::Youtube(url, title, channel, tags) => {
                let template_file = root_dir.join(".zk/templates/yt_video.md");
                let Ok(zk_file) = TaskData::get_zk_file(title, template_file) else {
                    return false;
                };
                let pd = zk_parsing::parse_zk_file(zk_file);
                println!("{pd:?}");
                true
            }
            _ => todo!("not implemented: conversion of {self:?} to zk."),
        }
    }

    fn get_zk_file(title: &str, template_path: PathBuf) -> Result<PathBuf> {
        use std::process::Command;
        let output = Command::new("zk")
            .arg("new")
            .arg("--no-input")
            .arg(format!("--title={title}"))
            .arg(format!("--template={}", template_path.to_str().unwrap()))
            .arg("-p")
            .output()?;
        let p = std::str::from_utf8(&output.stdout)?;
        Ok(PathBuf::from_str(p)?)
    }
    fn add_to_logseq_journal(
        &self,
        templates: &LogSeqTemplates,
        journal_file: &mut ParsedDocument,
    ) -> bool {
        use crate::document_component::DocumentElement::ListElement;
        match self {
            TaskData::Youtube(url, title, channel, tags) => {
                let mut yt_template = templates
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
                journal_file.add_component(yt_template);
            }
            TaskData::Sbs(url, author, title, tags) => {
                if let Some(comp) = templates.get_template_comp("article") {
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
                        journal_file.add_component(comp);
                    }
                }
            }
            TaskData::YtPlaylist(url, channel, title) => {
                let mut temp = templates.get_template_comp("youtube_playlist").unwrap();
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
                    journal_file.add_component(temp);
                }
            }
            TaskData::Interactive(template_name, url, title, tags) => {
                let mut comp = templates.get_template_comp(template_name).unwrap();
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
                    journal_file.add_component(comp);
                }
            }
            _ => {
                return false;
            }
        }
        true
    }
}

fn handle_youtube_task(task: &TodoistTask, config: &Config) -> TaskData {
    let yt_video_url_re =
        Regex::new(r"(https://)(?:www\.)?(?:youtu.be|youtube\.com)/(shorts/)?[A-Za-z0-9?=\-_]*")
            .unwrap();
    if let Some(m) = yt_video_url_re.captures(&task.content) {
        if let Some(video_url) = m.get(0) {
            let video_url = video_url.as_str();
            if let Ok((video_title, authors)) = youtube_details(video_url, &config.yt_api_key) {
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

fn handle_sbs_task(task: &TodoistTask) -> TaskData {
    let sbs_link_re =
        Regex::new(r"https://ckarchive\.com/b/[a-zA-Z0-9]*\?ck_subscriber_id=2334581400").unwrap();
    let author_re = Regex::new(r" newsletter is by ([a-zA-Z\.\s]*).&lt;/h3&gt;").unwrap();

    if sbs_link_re.captures(&task.content).is_some() {
        let article_url = &task.content;

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
        let title = if let (Some(start), Some(end)) = (text.find("<title>"), text.find("</title>"))
        {
            Some(text[start + 7..end].to_string())
        } else {
            None
        };
        let tags = vec!["#Fitness".to_string()];
        return TaskData::Sbs(article_url.clone(), author, title, tags);
    }
    TaskData::Unhandled
}

fn handle_youtube_playlist(task: &TodoistTask, config: &Config) -> TaskData {
    let playlist_re = Regex::new(r"https://www\.youtube\.com/playlist\?list=[a-zA-Z0-9]+").unwrap();
    if playlist_re.captures(&task.content).is_some() {
        let playlist_url = task.content.clone();
        if let Ok((description, channel)) =
            youtube_playlist_details(&playlist_url, &config.yt_api_key)
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

                if let Ok((video_title, authors)) = youtube_details(video_url, &config.yt_api_key) {
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
                    youtube_playlist_details(playlist_url, &config.yt_api_key)
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
