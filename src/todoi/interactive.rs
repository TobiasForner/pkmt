use anyhow::{bail, Result};
use regex::Regex;

use crate::{document_component::ParsedDocument, todoi::todoi::fill_properties, util};

use super::{config::Config, todoi::LogSeqTemplates, todoist_api::TodoistTask};
#[derive(Debug)]
pub enum Resolution {
    Skip,
    Complete,
    Cancel,
}

pub fn handle_interactive(
    task: &TodoistTask,
    journal_file: &mut ParsedDocument,
    templates: &LogSeqTemplates,
    config: &Config,
) -> Resolution {
    use crate::document_component::DocumentElement::ListElement;
    use Resolution::*;
    let template_names = templates.template_names();

    println!("{}", task.content);
    println!("Please choose the template to use:");
    template_names
        .iter()
        .enumerate()
        .for_each(|(i, n)| println!("{i}: '{n}'"));

    // wait for user to choose
    let choice = loop {
        let answer =
            get_user_input("Please enter your choice (c to cancel for all, s to skip this task)");
        let Ok(answer) = answer else {
            panic!("error!");
        };

        println!("answer: {answer:?}");
        match answer.as_str() {
            "c" => return Cancel,
            "s" => return Skip,
            n => {
                if let Ok(num) = n.parse::<usize>() {
                    break num;
                }
            }
        }
    };

    let mut comp = templates
        .get_template_comp(&template_names[choice])
        .unwrap();

    println!("Chose {choice}: {}", template_names[choice]);
    if let ListElement(_, props) = comp.get_element_mut() {
        let mut add = vec![];
        let content = util::apply_substitutions(&task.content);
        let url_re =
            Regex::new(r"\[([\sa-zA-ZüäöÜÄÖ0-9'?!\.:\-/|·$§]+)\]\(([\sa-zA-Z0-9'?!\.:\-/_]+)\)");
        if let Some(captures) = url_re.unwrap().captures(&content) {
            if let Some(title) = captures.get(1) {
                let title = title.as_str().to_string();
                let tags = config.get_keyword_tags(&title);
                add.push(("description", vec![title]));
                add.push(("tags", tags));
            } else {
                println!("No title capture: {content}");
            }
            if props.iter().any(|(k, _)| k == "url") {
                if let Some(url) = captures.get(2) {
                    add.push(("url", vec![url.as_str().to_string()]))
                } else {
                    println!("No url capture: {content}");
                }
            }
        } else {
            println!("No match: {:?}", task.content);
        }
        let new_props = fill_properties(props, &add, &["template"]);
        *props = new_props;
    }
    journal_file.add_component(comp);
    Complete
}
fn get_user_input(prompt: &str) -> Result<String> {
    println!("{prompt}: ");
    let mut answer = Default::default();
    if std::io::stdin().read_line(&mut answer).is_ok() {
        Ok(answer.trim().to_string())
    } else {
        bail!("Failed to get input!")
    }
}
