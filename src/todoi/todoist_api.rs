use anyhow::{Context, Result};
use serde::Deserialize;
/*
{"id": "2168048243",
                "parent_id": null,
                "order": 0,
                "color": "grey",
                "name": "Inbox",
                "comment_count": 0,
                "is_shared": false,
                "is_favorite": false,
                "is_inbox_project": true,
                "is_team_inbox": false,
                "url": "https://todoist.com/showProject?id=2168048243",
                "view_style": "list"
}
*/
#[derive(Deserialize, Debug)]
pub struct TodoistProject {
    id: String,
    is_inbox_project: bool,
}

/*
{
        "creator_id": "2671355",
        "created_at": "2019-12-11T22:36:50.000000Z",
        "assignee_id": "2671362",
        "assigner_id": "2671355",
        "comment_count": 10,
        "is_completed": false,
        "content": "Buy Milk",
        "description": "",
        "due": {
            "date": "2016-09-01",
            "is_recurring": false,
            "datetime": "2016-09-01T12:00:00.000000Z",
            "string": "tomorrow at 12",
            "timezone": "Europe/Moscow"
        },
        "duration": null,
        "id": "2995104339",
        "labels": ["Food", "Shopping"],
        "order": 1,
        "priority": 1,
        "project_id": "2203306141",
        "section_id": "7025",
        "parent_id": "2995104589",
        "url": "https://todoist.com/showTask?id=2995104339"
    },
*/
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TodoistTask {
    id: String,
    pub content: String,
}

pub struct TodoistAPI {
    todoist_api_key: String,
    runtime: tokio::runtime::Runtime,
}

impl TodoistAPI {
    pub fn new(todoist_api_key: &str) -> Self {
        Self {
            todoist_api_key: todoist_api_key.to_string(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        }
    }

    pub fn get_inbox(&self) -> Result<TodoistProject> {
        let tmp = self
            .get_all_projects()
            .into_iter()
            .find(|p| p.is_inbox_project);
        tmp.context("Inbox does not exist!")
    }

    pub fn get_project_tasks(&self, project: &TodoistProject) -> Result<Vec<TodoistTask>> {
        let res = self
            .req_base("https://api.todoist.com/rest/v2/tasks")
            .query(&[("project_id", &project.id)])
            .send();
        let res = self.runtime.block_on(res)?;
        let text = self.runtime.block_on(res.text())?;
        serde_json::from_str(&text).context(format!("Could not parse {text}"))
    }

    pub fn close_task(&self, task: &TodoistTask) -> bool {
        let res = self.req_base_post(&format!(
            "https://api.todoist.com/rest/v2/tasks/{}/close",
            task.id
        ));
        let res = self.runtime.block_on(res.send()).unwrap();
        res.status().as_u16() == 204
    }

    fn get_all_projects(&self) -> Vec<TodoistProject> {
        let url = "https://api.todoist.com/rest/v2/projects";
        let req = self.req_base(url).try_clone().unwrap();

        let res = self.runtime.block_on(req.send()).unwrap();
        let text = self.runtime.block_on(res.text()).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn req_base(&self, url: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .get(url)
            .header("Authorization", format!("Bearer {}", self.todoist_api_key))
    }
    fn req_base_post(&self, url: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(url)
            .header("Authorization", format!("Bearer {}", self.todoist_api_key))
    }
}
