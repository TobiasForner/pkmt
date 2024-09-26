use std::{path::PathBuf, str::FromStr};

use anyhow::Result;
use regex::Regex;

use crate::{
    document_component::{DocumentComponent, ParsedDocument},
    logseq_parsing::parse_logseq_file,
    todoi::{
        config::Config,
        todoist_api::{TodoistAPI, TodoistTask},
        youtube_details::{youtube_details, youtube_playlist_details},
    },
};

struct LogSeqTemplates {
    templates_pd: ParsedDocument,
}
impl LogSeqTemplates {
    fn new(logseq_graph_root: &PathBuf) -> Result<Self> {
        let templates_file = logseq_graph_root
            .join("pages")
            .join("Templates.md")
            .canonicalize()
            .unwrap();

        let pd = parse_logseq_file(templates_file)?;
        Ok(Self { templates_pd: pd })
    }

    fn get_template_comp(&self, template_name: &str) -> Option<DocumentComponent> {
        use crate::document_component::DocumentElement::ListElement;
        self.templates_pd
            .get_document_component(&|c| match &c.element {
                ListElement(_, props) => props
                    .iter()
                    .any(|(key, value)| key == "template" && value == template_name),
                _ => false,
            })
    }
}

pub fn main(logseq_graph_root: PathBuf, complete_tasks: bool) -> Result<()> {
    let config = Config::parse(PathBuf::from_str(
        "/mnt/c/Users/Tobias/AppData/Local/todoist_import/todoist_import/todoi_config.toml",
    )?);
    let todoist_api = TodoistAPI::new(&config.todoist_api_key);
    let inbox = todoist_api.get_inbox()?;

    let inbox_tasks = todoist_api.get_project_tasks(&inbox)?;

    let today = chrono::offset::Local::now();
    let todays_journal_file = logseq_graph_root
        .join("journals")
        .join(today.format("%Y_%m_%d.md").to_string());
    let todays_journal = if todays_journal_file.exists() {
        println!("loaded existing journal file");
        parse_logseq_file(&todays_journal_file).unwrap()
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
        .filter(|task| !new_completions.contains(task))
        .collect();

    let mut new_completions =
        handle_youtube_playlists(&remaining_tasks, &templates, &mut todays_journal, &config);
    completed_tasks.append(&mut new_completions);
    let remaining_tasks: Vec<TodoistTask> = remaining_tasks
        .into_iter()
        .filter(|task| !new_completions.contains(task))
        .collect();

    println!("remaining: {remaining_tasks:?}");

    std::fs::write(todays_journal_file, todays_journal.to_logseq_text(&None))?;
    if complete_tasks {
        completed_tasks.iter().for_each(|t| {
            println!("Completing: {}", t.content);
            todoist_api.close_task(t);
        });
    }
    Ok(())
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
                let (authors, video_title) = {
                    if let Ok((video_title, authors)) =
                        youtube_details(video_url, &config.yt_api_key)
                    {
                        (authors, video_title)
                    } else {
                        (String::new(), String::new())
                    }
                };

                let mut tags = vec![];
                if let Some(mut ct) = config.get_channel_tags(&authors) {
                    tags.append(&mut ct);
                }

                tags.append(&mut config.get_keyword_tags(&video_title));
                tags.sort();
                tags.dedup();
                *props = fill_properties(
                    props,
                    &[
                        ("tags", &tags),
                        ("description", &[video_title]),
                        ("authors", &[authors]),
                    ],
                    &["template"],
                );

                // add embed
                let embed_block = yt_template
                    .get_nth_child_mut(0)
                    .unwrap()
                    .get_nth_child_mut(0)
                    .unwrap();

                let pd = ParsedDocument::ParsedText(vec![DocumentComponent::new_text(&format!(
                    "{{{{video {video_url}}}}}"
                ))]);
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
                            ("description", &[description]),
                            ("authors", &[channel]),
                            ("url", &[playlist_url.to_string()]),
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

fn fill_properties(
    props: &[(String, String)],
    add: &[(&str, &[String])],
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
        Regex::new(r"https://ckarchive.com/b/[a-zA-Z0-9]*\?ck_subscriber_id=2334581400").unwrap();
    let author_re = Regex::new(r" newsletter is by ([a-zA-Z\.\s]*).&lt;/h3&gt;").unwrap();

    if let Some(comp) = templates.get_template_comp("article") {
        tasks
            .iter()
            .filter_map(|t| sbs_link_re.captures(&t.content).map(|c| (t, c)))
            .filter_map(|(t, c)| c.get(0).map(|m| (t, m)))
            .filter_map(|(task, text)| {
                let mut comp = comp.clone();
                let text = text.as_str();

                if let ListElement(_, props) = comp.get_element_mut() {
                    let mut source = vec!["[[Stronger by Science]]".to_string()];
                    if let Some(author) = author_re.captures(text) {
                        source.push(author.get(0).unwrap().as_str().to_string());
                    }
                    *props = fill_properties(props, &[("source", &source)], &["template"]);
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
