pub mod config;
pub mod handlers;
mod interactive;
mod todoist_api;
mod youtube_details;
use scraper::{Html, Selector};
use std::{fmt::Debug, path::PathBuf, vec};

use anyhow::Result;
use interactive::get_interactive_data;
use regex::Regex;
use tracing::{debug, info, instrument};

use crate::{
    document_component::{DocumentComponent, ListElem, ParsedDocument, PropValue},
    parse::{TextMode, parse_all_files_in_dir},
    todoi::{
        config::Config,
        handlers::handle_tasks_main,
        interactive::Resolution,
        todoist_api::{TodoistAPI, TodoistTask},
        youtube_details::{youtube_details, youtube_playlist_details},
    },
};

pub fn get_list_elem_with_doc_elem(
    pd: &ParsedDocument,
    elem_selector: &dyn Fn(&DocumentComponent) -> bool,
) -> Option<ListElem> {
    pd.get_list_elem(&|le| le.contents.components().iter().any(elem_selector))
}

/// gathers tasks and calls the correct handler
/// tasks are marked as completed if complete_tasks is set
pub fn main(root_dir: PathBuf, complete_tasks: bool, mode: TextMode) -> Result<()> {
    let config = Config::load()?;
    let todoist_api = TodoistAPI::new(&config.keys.todoist_api_key);
    let inbox = todoist_api.get_inbox()?;

    let mut inbox_tasks = todoist_api.get_project_tasks(&inbox)?;
    inbox_tasks = todoist_api.get_lonely_tasks(&inbox_tasks);
    inbox_tasks.sort_by_key(|t| t.content.clone());
    info!("Retrieved todoist tasks.");
    inbox_tasks.dedup_by_key(|t| t.content.clone());
    debug!("mode: {mode:?}");
    let completed_tasks = handle_tasks_main(&inbox_tasks, &config, mode, &root_dir)?;

    if complete_tasks {
        completed_tasks.iter().for_each(|t| {
            let success = todoist_api.close_task(t);
            if success {
                println!("Marked task '{}' as completed", t.content);
            } else {
                println!("ERROR: Failed to Mark task '{}' as completed!", t.content);
            }
        });
    }
    Ok(())
}

/// Adds the the given values to the first property in the pd with the given name. Does nothing if the property
/// is not found
#[instrument]
fn fill_all_props_le(pd: &mut ListElem, properties: &[(&str, Vec<PropValue>)]) {
    properties.iter().for_each(|(prop_name, values)| {
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
    });
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

#[derive(Debug)]
pub enum TaskData {
    Unhandled,
    /// url, title, channel, tags
    Youtube(String, String, String, Vec<String>),
    /// url, optional author, optional title, tags, optional description
    Sbs(
        String,
        Option<String>,
        Option<String>,
        Vec<String>,
        Option<String>,
    ),
    /// url, channel, title
    YtPlaylist(String, String, String),
    /// template_name, optional url, optional title, tags, sources
    Interactive(
        String,
        Option<String>,
        Option<String>,
        Vec<String>,
        Vec<String>,
    ),
}

impl TaskData {
    fn get_title(&self) -> Option<String> {
        use TaskData::*;
        match self {
            Youtube(_, title, _, _) => Some(title.to_string()),
            Sbs(_, _, title, _, _) => title.clone(),
            YtPlaylist(_, _, title) => Some(title.to_string()),
            Interactive(_, _, title, _, _) => title.clone(),
            _ => None,
        }
    }
    fn get_tags(&self) -> Vec<String> {
        use TaskData::*;
        match self {
            Unhandled => vec![],
            Youtube(_, _, _, tags) => tags.clone(),
            Sbs(_, _, _, tags, _) => tags.clone(),
            YtPlaylist(_, _, _) => vec![],
            Interactive(_, _, _, tags, _) => tags.clone(),
        }
    }

    fn get_url(&self) -> Option<&str> {
        use TaskData::*;
        match self {
            Unhandled => None,
            Youtube(url, _, _, _) => Some(url),
            Sbs(url, _, _, _, _) => Some(url),
            YtPlaylist(url, _, _) => Some(url),
            Interactive(_, url, _, _, _) => url.as_deref(),
        }
    }
}

fn handle_youtube_task(task: &TodoistTask, config: &Config) -> TaskData {
    let yt_video_url_re =
        Regex::new(r"(https://)(?:www\.)?(?:youtu.be|youtube\.com)/(shorts/)?[A-Za-z0-9?=\-_&]*")
            .unwrap();
    if let Some(m) = yt_video_url_re.captures(&task.content)
        && let Some(video_url) = m.get(0)
    {
        let video_url = video_url.as_str();
        if let Ok((video_title, authors)) = youtube_details(video_url, &config.keys.yt_api_key) {
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
    TaskData::Unhandled
}

#[instrument]
fn handle_sbs_task(task: &TodoistTask) -> TaskData {
    let sbs_link_re =
        Regex::new(r"https://ckarchive\.com/b/[a-zA-Z0-9]*\?ck_subscriber_id=2334581400").unwrap();
    let sbs_website_re = Regex::new(r"https://www.strongerbyscience.com/[0-9a-zA-Z-]+/").unwrap();

    let match_data = if let Some(art_url) = sbs_link_re.captures(&task.content) {
        let author_re = Regex::new(r" newsletter is by ([a-zA-Z\.\s]*).&lt;/h3&gt;").unwrap();
        Some((art_url.get(0), author_re))
    } else {
        let sbs_website_author_re =
            Regex::new("<meta name=\"author\" content=\"([a-zA-Z\\s\\-]+)\" />").unwrap();
        sbs_website_re
            .captures(&task.content)
            .map(|art_url| (art_url.get(0), sbs_website_author_re))
    };

    if let Some((Some(art_url), author_re)) = match_data {
        let article_url = art_url.as_str();
        debug!("found sbs website url {article_url}");
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

        let doc = Html::parse_document(&text);
        let selector = Selector::parse(".elementor-widget-theme-post-excerpt").unwrap();
        let mut selection = doc.select(&selector);
        let desc = if let Some(n) = selection.next() {
            let mut description = String::new();
            n.text().for_each(|t| description.push_str(t.trim()));
            Some(description)
        } else {
            None
        };

        let title = if let (Some(start), Some(end)) = (text.find("<title>"), text.find("</title>"))
        {
            let title = text[start + 7..end].trim_end_matches(" &#8226; Stronger by Science");
            Some(title.to_string())
        } else {
            None
        };
        let tags = vec!["fitness".to_string()];
        let res = TaskData::Sbs(article_url.to_string(), author, title, tags, desc);
        debug!("found {res:?} for {task:?}");
        return res;
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

fn url_is_duplicate(url: &str, root_dir: &PathBuf, mode: &TextMode) -> Result<bool> {
    let parsed_documents = parse_all_files_in_dir(root_dir, mode)?;
    let mut res = false;
    parsed_documents.iter().for_each(|pd| {
        if pd
            .get_document_component(&|dc: &DocumentComponent| {
                if let DocumentComponent::Properties(props) = dc {
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
