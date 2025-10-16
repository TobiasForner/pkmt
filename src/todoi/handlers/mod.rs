use std::path::PathBuf;

use crate::{
    document_component::{DocumentComponent, PropValue},
    parse::{TextMode, parse_all_files_in_dir},
    todoi::{
        TaskData,
        config::Config,
        get_task_data_full,
        handlers::{logseq_handler::LogSeqHandler, zk_handler::ZkHandler},
        todoist_api::TodoistTask,
    },
};
use anyhow::Result;
use tracing::debug;
use tracing::instrument;

pub mod logseq_handler;
pub mod zk_handler;
pub trait TaskDataHandler {
    fn handle_task_data(&mut self, task_data: &TaskData) -> Result<bool>;
    fn get_template_names(&self) -> Result<Vec<String>>;
}

#[instrument(skip_all)]
pub fn handle_tasks_main(
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
    let all_urls = get_all_urls(root_dir, mode)?;
    let deduped_tasks: Vec<TodoistTask> = tasks
        .iter()
        .filter_map(|t| {
            if all_urls.iter().any(|u| t.content.contains(u)) {
                println!("Found DUPLICATE task: {}", t.content);
                None
            } else {
                Some(t.clone())
            }
        })
        .collect();
    let tasks = get_task_data_full(&deduped_tasks, config, &handler.get_template_names()?);

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

fn get_all_urls(root_dir: &PathBuf, mode: TextMode) -> Result<Vec<String>> {
    let parsed_documents = parse_all_files_in_dir(root_dir, &mode)?;
    let prop_dcs: Vec<DocumentComponent> = parsed_documents
        .iter()
        .flat_map(|pd| {
            pd.get_all_document_components(&|dc: &DocumentComponent| {
                if let DocumentComponent::Properties(props) = dc {
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
            if let DocumentComponent::Properties(props) = dc {
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
