use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::util::{FileLocation, FileStorage};

/*
{
  "results": [
    {
      "id": "string",
      "can_assign_tasks": true,
      "child_order": 0,
      "color": "string",
      "creator_uid": "string",
      "created_at": "string",
      "is_archived": true,
      "is_deleted": true,
      "is_favorite": true,
      "is_frozen": true,
      "name": "string",
      "updated_at": "string",
      "view_style": "string",
      "default_order": 0,
      "description": "string",
      "public_key": "string",
      "access": {
        "visibility": "restricted",
        "configuration": {}
      },
      "role": "string",
      "parent_id": "string",
      "inbox_project": true,
      "is_collapsed": true,
      "is_shared": true
    }
  ],
  "next_cursor": "string"
}
*/
#[derive(Deserialize, Debug)]
pub struct TodoistProject {
    id: String,
    inbox_project: bool,
}

/*
{
  "content": "string",
  "description": "string",
  "project_id": "6XGgm6PHrGgMpCFX",
  "section_id": "6fFPHV272WWh3gpW",
  "parent_id": "6XGgmFVcrG5RRjVr",
  "order": 12,
  "labels": [
    "string"
  ],
  "priority": 2,
  "assignee_id": 123456789,
  "due_string": "string",
  "due_date": "string",
  "due_datetime": "string",
  "due_lang": "string",
  "duration": 30,
  "duration_unit": "minute",
  "deadline_date": "2025-02-12"
}
*/
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TodoistTask {
    id: String,
    pub content: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct TodoistAPIData {
    inbox_id: Option<String>,
}

impl FileLocation for TodoistAPIData {
    fn get_file_location() -> PathBuf {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt").unwrap();
        dirs.config_local_dir().join("todoist_api_data.toml")
    }
}

/// wrapper for the return object of the todoist api v1
#[derive(Deserialize, Debug)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
struct TodoistAPIResult<T>
where
    T: DeserializeOwned,
{
    results: Vec<T>,
}

pub struct TodoistAPI {
    todoist_api_key: String,
    api_data: TodoistAPIData,
    runtime: tokio::runtime::Runtime,
}

impl TodoistAPI {
    pub fn new(todoist_api_key: &str) -> Self {
        let api_data = TodoistAPIData::load_or_new();
        Self {
            todoist_api_key: todoist_api_key.to_string(),
            api_data,
            runtime: tokio::runtime::Runtime::new().unwrap(),
        }
    }

    pub fn get_inbox(&mut self) -> Result<TodoistProject> {
        if let Some(inbox_id) = &self.api_data.inbox_id {
            self.get_project_by_id(inbox_id)
        } else {
            let inbox_candidates = self
                .search_projects("Inbox")?
                .into_iter()
                .find(|p| p.inbox_project);
            let inbox = inbox_candidates.context("Inbox does not exist!")?;
            self.api_data.inbox_id = Some(inbox.id.clone());
            _ = self.api_data.store();
            Ok(inbox)
        }
    }
    pub fn get_project_by_id(&self, project_id: &str) -> Result<TodoistProject> {
        let res = self
            .relative_api_get_req(&format!("/projects/{project_id}"))
            .send();
        let res = self.runtime.block_on(res)?;
        if res.status() != 200 {
            println!("ERROR: failed to retrieve Todoist project with id {project_id}!",);
        }
        let text = self.runtime.block_on(res.text())?;
        let result: TodoistProject =
            serde_json::from_str(&text).context("failed to parse response for project")?;
        Ok(result)
    }

    pub fn get_inbox_tasks(&mut self) -> Result<Vec<TodoistTask>> {
        if let Some(inbox_id) = &self.api_data.inbox_id {
            self.get_project_tasks(inbox_id)
        } else {
            let inbox_project = self.get_inbox()?;
            self.get_project_tasks(&inbox_project.id)
        }
    }

    pub fn get_project_tasks(&self, project_id: &str) -> Result<Vec<TodoistTask>> {
        let res = self
            .relative_api_get_req("/tasks")
            .query(&[("project_id", project_id)])
            .send();
        let res = self.runtime.block_on(res)?;
        if res.status() != 200 {
            println!("ERROR: failed to retrieve Todoist tasks for project {project_id}!",);
        }
        let text = self.runtime.block_on(res.text())?;
        let results: TodoistAPIResult<TodoistTask> =
            serde_json::from_str(&text).context("failed to parse response for project tasks")?;
        Ok(results.results)
    }

    pub fn get_lonely_tasks(&self, tasks: &[TodoistTask]) -> Vec<TodoistTask> {
        let ids_to_filter: HashSet<String> = tasks
            .iter()
            .filter_map(|t| {
                t.parent_id
                    .as_ref()
                    .map(|parent_id| (t.id.clone(), parent_id.clone()))
            })
            .flat_map(|(a, b)| [a.to_string(), b.to_string()])
            .collect();
        tasks
            .iter()
            .filter(|t| !ids_to_filter.contains(&t.id))
            .cloned()
            .collect()
    }

    pub fn close_task(&self, task: &TodoistTask) -> bool {
        let res = self.relative_api_post_req(&format!("/tasks/{}/close", task.id));
        let res = self.runtime.block_on(res.send()).unwrap();
        res.status().as_u16() == 204
    }

    fn search_projects(&self, query: &str) -> Result<Vec<TodoistProject>> {
        let req = self
            .relative_api_get_req("/projects/search")
            .query(&[("query", query)]);

        let res = self.runtime.block_on(req.send())?;
        if res.status() != 200 {
            println!(
                "ERROR: failed to retrieve projects from Todoist: status {}",
                res.status()
            );
        }
        let text = self.runtime.block_on(res.text())?;

        let result: TodoistAPIResult<TodoistProject> =
            serde_json::from_str(&text).context("failed to parse response for search projects")?;
        Ok(result.results)
    }

    fn _get_all_projects(&self) -> Result<Vec<TodoistProject>> {
        let req = self
            .relative_api_get_req("/projects")
            .try_clone()
            .context("Failed to clone todoist projects url")?;

        let res = self.runtime.block_on(req.send())?;
        if res.status() != 200 {
            println!(
                "ERROR: failed to retrieve projects from Todoist: status {}",
                res.status()
            );
        }
        let text = self.runtime.block_on(res.text())?;

        let result: TodoistAPIResult<TodoistProject> =
            serde_json::from_str(&text).context("failed to parse response for get all projects")?;
        Ok(result.results)
    }

    fn relative_api_get_req(&self, rel: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .get(format!("https://api.todoist.com/api/v1{rel}"))
            .header("Authorization", format!("Bearer {}", self.todoist_api_key))
    }

    fn relative_api_post_req(&self, rel: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("https://api.todoist.com/api/v1{rel}"))
            .header("Authorization", format!("Bearer {}", self.todoist_api_key))
    }
}
