use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::util;

use super::{config::Config, todoist_api::TodoistTask, TaskData};
#[derive(Debug)]
pub enum Resolution {
    ToHandle,
    Skip,
    Cancel,
}

pub fn get_interactive_data(
    task: &TodoistTask,
    template_names: &[String],
    config: &Config,
) -> (Resolution, TaskData) {
    use Resolution::*;
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
            "c" => return (Cancel, TaskData::Unhandled),
            "s" => return (Skip, TaskData::Unhandled),
            n => {
                if let Ok(num) = n.parse::<usize>() {
                    break num;
                }
            }
        }
    };
    let template_name = &template_names[choice];

    println!("Chose {choice}: {template_name}");
    let content = util::apply_substitutions(&task.content);
    let url_re = url_re().unwrap();
    if let Some(captures) = url_re.captures(&content) {
        let mut tags = vec![];
        let title = if let Some(title) = captures.get(1) {
            let title = title.as_str().to_string();
            tags = config.get_keyword_tags(&title);
            Some(title)
        } else {
            println!("No title capture: {content}");
            None
        };

        let mut sources = vec![];
        let url = if let Some(url) = captures.get(2) {
            let url = url.as_str().to_string();
            let url_tags = config.get_url_tags(&url);
            url_tags.into_iter().for_each(|ut| {
                if !tags.contains(&ut) {
                    tags.push(ut);
                }
            });

            sources = config.get_url_sources(&url);
            Some(url)
        } else {
            println!("No url capture: {content}");
            None
        };
        (
            ToHandle,
            TaskData::Interactive(template_name.clone(), url.clone(), title, tags, sources),
        )
    } else {
        println!("No url match: {content:?} with {url_re:?}");
        (Skip, TaskData::Unhandled)
    }
}

fn url_re() -> Result<Regex> {
    let url_re = Regex::new(
        r####"\[((?:[\sa-zA-ZüäöÜÄÖ0-9'’’?!\.:\-/|•·$§@&+,()\\{}\[\]#"]|[^\u0000-\u007F])+)\]\(([\sa-zA-Z0-9'?!\.:\-/_=%&@#]+)\)"####,
    );
    url_re.context("failed to construct url_re")
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
