use std::{collections::HashSet, path::PathBuf};

use crate::{
    document_component::{DocumentComponent, PropValue},
    parsing::{TextMode, parse_all_files_in_dir_iter},
    todoi::{
        TaskData,
        config::Config,
        get_task_data_full,
        handlers::{logseq_handler::LogSeqHandler, zk_handler::ZkHandler},
        todoist_api::TodoistTask,
    },
    util::{FileLocation, FileStorage},
};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressIterator, ProgressStyle};
use serde::{Deserialize, Serialize};
use tracing::debug;
use tracing::instrument;

pub mod logseq_handler;
pub mod zk_handler;
pub trait TaskDataHandler {
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool>;
    fn get_template_names(&self) -> Result<Vec<String>>;
}

#[instrument(skip_all)]
pub fn handle_tasks(
    tasks: &[TodoistTask],
    config: &Config,
    mode: TextMode,
    root_dir: &PathBuf,
) -> Result<Vec<TodoistTask>> {
    let mut handler: Box<dyn TaskDataHandler> = match mode {
        TextMode::Zk => Box::new(ZkHandler::new(root_dir.to_path_buf())),
        TextMode::LogSeq => Box::new(LogSeqHandler::new(root_dir.to_path_buf())?),
        _ => todo!(),
    };
    let mut url_cache = UrlCache::load_or_new();
    if url_cache.urls.is_empty() {
        url_cache.reload_urls(&mode, root_dir)?;
    }
    let deduped_tasks: Vec<TodoistTask> = tasks
        .iter()
        .filter_map(|t| {
            if url_cache.urls.iter().any(|u| t.content.contains(u)) {
                println!("Found DUPLICATE task: {}", t.content);
                None
            } else {
                Some(t.clone())
            }
        })
        .collect();
    let tasks = get_task_data_full(&deduped_tasks, config, &handler.get_template_names()?);
    tasks.iter().for_each(|(td, _)| {
        if let Some(url) = td.get_url() {
            url_cache.urls.insert(url.to_string());
        }
    });
    _ = url_cache.store();

    let style = ProgressStyle::with_template("[{elapsed}] {msg} {bar}").unwrap();
    let bar = ProgressBar::new(tasks.len() as u64).with_style(style);
    bar.set_message("Importing tasks...");

    let tasks: Result<Vec<(bool, TodoistTask)>> = tasks
        .into_iter()
        .progress_with(bar)
        .map(|(td, task)| handler.handle_task_data(&td).map(|e| (e, task)))
        .collect();
    debug!("filtering handled tasks: {tasks:?}");
    let tasks = tasks?
        .iter()
        .filter_map(|(done, task)| if *done { Some(task.clone()) } else { None })
        .collect();
    Ok(tasks)
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct UrlCache {
    urls: HashSet<String>,
}

impl UrlCache {
    fn reload_urls(&mut self, mode: &TextMode, root_dir: &PathBuf) -> Result<()> {
        parse_all_files_in_dir_iter(root_dir, mode)?
            .filter_map(|pd| pd.ok())
            .flat_map(|pd| {
                println!("{:?}", pd.get_file_path());
                pd.get_all_document_components(&|dc: &DocumentComponent| {
                    if let DocumentComponent::Properties(props) = dc {
                        props.iter().any(|p| p.has_name("url"))
                    } else {
                        false
                    }
                })
                .into_iter()
            })
            .for_each(|dc| {
                if let DocumentComponent::Properties(props) = dc {
                    props.iter().filter(|p| p.has_name("url")).for_each(|p| {
                        p.values.iter().for_each(|v| {
                            if let PropValue::String(s) = v {
                                self.urls.insert(s.clone());
                            }
                        })
                    });
                }
            });
        Ok(())
    }
}

impl FileLocation for UrlCache {
    fn get_file_location() -> PathBuf {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt").unwrap();
        dirs.config_local_dir().join("url_cache.toml")
    }
}
