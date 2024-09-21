use std::path::PathBuf;

use anyhow::Result;
use regex::Regex;

use crate::{
    document_component::{DocumentElement, ParsedDocument},
    logseq_parsing::parse_logseq_file,
    todoi::todoist_api::{TodoistAPI, TodoistTask},
    todoi::youtube_details::youtube_details,
};

pub fn main(
    yt_api_key: &str,
    todoist_api_key: &str,
    logseq_graph_root: PathBuf,
    complete_tasks: bool,
) -> Result<()> {
    let todoist_api = TodoistAPI::new(todoist_api_key);
    let inbox = todoist_api.get_inbox()?;

    let inbox_tasks = todoist_api.get_project_tasks(&inbox)?;

    let templates_file = logseq_graph_root
        .join("pages")
        .join("Templates.md")
        .canonicalize()
        .unwrap();

    let pd = parse_logseq_file(templates_file)?;

    let today = chrono::offset::Local::now();
    let todays_journal_file = logseq_graph_root
        .join("journals")
        .join(today.format("%Y_%m_%d.md").to_string());
    let mut todays_journal = if todays_journal_file.exists() {
        parse_logseq_file(todays_journal_file).unwrap()
    } else {
        ParsedDocument::ParsedFile(vec![], todays_journal_file)
    };

    let tasks = handle_youtube_tasks(
        yt_api_key,
        &inbox_tasks,
        &pd,
        &mut todays_journal,
        complete_tasks,
        &todoist_api,
    );

    println!("remaining: {tasks:?}");

    println!("final pd\n{}", todays_journal.to_logseq_text(&None));

    Ok(())
}

fn handle_youtube_tasks(
    yt_api_key: &str,
    tasks: &[TodoistTask],
    templates_pd: &ParsedDocument,
    journal_file: &mut ParsedDocument,
    complete_tasks: bool,
    todoist_api: &TodoistAPI,
) -> Vec<TodoistTask> {
    use crate::document_component::DocumentElement::ListElement;
    let yt_template_comp = templates_pd
        .get_document_component(&|c| match &c.element {
            ListElement(_, props) => props
                .iter()
                .any(|(key, value)| key == "template" && value == "youtube"),
            _ => false,
        })
        .unwrap();

    let yt_video_url_re =
        Regex::new(r"(https://)(?:www\.)?(?:youtu.be|youtube\.com)/(shorts/)?[A-Za-z0-9?=\-_]*")
            .unwrap();

    let mut remaining_tasks = vec![];

    tasks.iter().for_each(|task| {
        if let Some(capture) = yt_video_url_re.captures(&task.content) {
            let video_url = capture.get(0).unwrap().as_str();

            let mut yt_template_comp = yt_template_comp.clone();

            // remove template tag and add new tags
            if let ListElement(_, props) = yt_template_comp.get_element_mut() {
                let mut new_props = vec![];
                let (authors, video_title) = {
                    if let Ok((video_title, authors)) = youtube_details(video_url, yt_api_key) {
                        (format!("[[{authors}]]"), video_title)
                    } else {
                        (String::new(), String::new())
                    }
                };

                props.iter().for_each(|(key, val)| match key.as_str() {
                    "tags" => {
                        new_props.push((key.clone(), format!("{val}, other_tag")));
                    }
                    "template" => {}
                    "authors" => new_props.push(("authors".to_string(), authors.clone())),
                    "description" => {
                        new_props.push(("descriptions".to_string(), video_title.clone()))
                    }
                    _ => new_props.push((key.to_string(), val.to_string())),
                });
                *props = new_props;

                // add embed
                let embed_block = yt_template_comp
                    .get_nth_child_mut(0)
                    .unwrap()
                    .get_nth_child_mut(0)
                    .unwrap();
                embed_block.element = DocumentElement::Text(format!("{{{{video {video_url}}}}}"));
            }

            journal_file.add_component(yt_template_comp);
            if complete_tasks {
                todoist_api.close_task(task);
            }
        } else {
            remaining_tasks.push(task.clone());
        }
    });
    remaining_tasks
}
