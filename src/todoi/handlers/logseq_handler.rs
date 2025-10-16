use crate::document_component::MentionedFile;
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
    vec,
};

use anyhow::{Context, Result};

use crate::todoi::{
    TaskData, fill_all_props_le, get_list_elem_with_doc_elem, handlers::TaskDataHandler,
};
use crate::{
    document_component::{DocumentComponent, ListElem, ParsedDocument, PropValue},
    logseq_parsing::parse_logseq_file,
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

    /// returns the list element containing the properties matching the template name
    pub fn get_template_comp(&self, template_name: &str) -> Option<ListElem> {
        get_list_elem_with_doc_elem(&self.templates_pd, &|elem| match elem {
            DocumentComponent::Properties(props) => props.iter().any(|p| {
                p.has_name("template") && p.has_value(&PropValue::String(template_name.to_string()))
            }),
            _ => false,
        })
    }

    pub fn template_names(&self) -> Vec<String> {
        let mut res = vec![];
        self.templates_pd
            .get_all_document_components(&|c: &DocumentComponent| match &c {
                DocumentComponent::Properties(props) => {
                    props.iter().any(|p| p.has_name("template"))
                }
                _ => false,
            })
            .iter()
            .for_each(|c| {
                if let DocumentComponent::Properties(props) = &c
                    && let Some(p) = props.iter().find(|p| p.has_name("template"))
                {
                    p.values.iter().for_each(|v| {
                        let tm = match v {
                            PropValue::FileLink(mf, _, _) => mf.to_string(),
                            PropValue::String(text) => text.to_string(),
                        };
                        res.push(tm);
                    });
                }
            });
        res
    }
}

#[derive(Debug)]
pub struct LogSeqHandler {
    templates: LogSeqTemplates,
    todays_journal: ParsedDocument,
    todays_journal_file: PathBuf,
}

impl LogSeqHandler {
    pub fn new(graph_root: PathBuf) -> Result<Self> {
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
        use TaskData::*;
        match task_data {
            Youtube(url, title, channel, tags) => {
                // retrieve the youtube template frmo the templates file
                // then fill in the properties
                // then add a child list item with the youtube embed (or fall back to simply adding
                // a yt link)
                let mut yt_template = self
                    .templates
                    .get_template_comp("youtube")
                    .expect("No youtube template!")
                    .clone();
                let properties = [
                    (
                        "authors",
                        vec![PropValue::FileLink(
                            MentionedFile::FileName(channel.to_string()),
                            None,
                            None,
                        )],
                    ),
                    ("description", vec![PropValue::String(title.clone())]),
                    (
                        "tags",
                        tags.iter()
                            .map(|t| PropValue::String(t.to_string()))
                            .collect(),
                    ),
                ];
                fill_all_props_le(&mut yt_template, &properties);

                // embed child
                if let Some(le) = yt_template.children.get_mut(0)
                    && let Some(le) = le.children.get_mut(0)
                {
                    let embed = if url.contains("/shorts/") {
                        DocumentComponent::Text(url.to_string())
                    } else {
                        DocumentComponent::Text(format!("{{{{video {url}}}}}"))
                    };
                    le.contents.add_component(embed);
                }
                let yt_block = DocumentComponent::List(vec![yt_template], false);
                self.todays_journal.add_component(yt_block);
            }
            TaskData::Sbs(url, author, title, tags, description) => {
                if let Some(comp) = self.templates.get_template_comp("article") {
                    let mut comp = comp.clone();
                    let mut source = vec![PropValue::String("[[Stronger by Science]]".to_string())];
                    if let Some(author) = author {
                        source.push(PropValue::String(author.clone()));
                    }
                    let url = vec![PropValue::String(url.clone())];

                    let mut desc = vec![];
                    if let Some(description) = description {
                        desc.push(PropValue::String(description.to_string()));
                    }

                    let mut properties: Vec<(&str, Vec<PropValue>)> = vec![
                        ("source", source),
                        ("url", url),
                        (
                            "tags",
                            tags.iter()
                                .map(|t| PropValue::String(t.to_string()))
                                .collect(),
                        ),
                        ("description", desc),
                    ];
                    if let Some(title) = title {
                        properties.push(("description", vec![PropValue::String(title.clone())]));
                    }
                    fill_all_props_le(&mut comp, &properties);
                    let comp = DocumentComponent::List(vec![comp], false);
                    self.todays_journal.add_component(comp);
                }
            }
            TaskData::YtPlaylist(url, channel, title) => {
                let mut temp = self
                    .templates
                    .get_template_comp("youtube_playlist")
                    .unwrap();
                let properties = &[
                    ("description", vec![PropValue::String(title.to_string())]),
                    ("authors", vec![PropValue::String(format!("[[{channel}]]"))]),
                    ("url", vec![PropValue::String(url.to_string())]),
                ];
                fill_all_props_le(&mut temp, properties);
                let list = DocumentComponent::List(vec![temp], false);
                self.todays_journal.add_component(list);
            }
            TaskData::Interactive(template_name, url, title, tags, sources) => {
                let mut comp = self.templates.get_template_comp(template_name).unwrap();
                let mut add = vec![];
                if let Some(title) = title {
                    add.push(("description", vec![title.clone()]));
                }
                add.push(("tags", tags.clone()));

                let mut properties: Vec<(&str, Vec<PropValue>)> = vec![
                    (
                        "source",
                        sources
                            .iter()
                            .map(|s| PropValue::String(s.to_string()))
                            .collect(),
                    ),
                    (
                        "tags",
                        tags.iter()
                            .map(|t| PropValue::String(t.to_string()))
                            .collect(),
                    ),
                ];
                if let Some(url) = url {
                    properties.push(("url", vec![PropValue::String(url.to_string())]))
                }
                fill_all_props_le(&mut comp, &properties);
                let list = DocumentComponent::List(vec![comp], false);
                self.todays_journal.add_component(list);
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
