use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub yt_api_key: String,
    pub todoist_api_key: String,
    yt_tag: Vec<ChannelTags>,
    kw_tag: Vec<KeywordTags>,
}

impl Config {
    pub fn parse(config_path: PathBuf) -> Self {
        let text = std::fs::read_to_string(config_path)
            .unwrap()
            .replace("\r\n", "\n");
        toml::from_str(&text).unwrap()
    }
    pub fn get_channel_tags(&self, channel: &str) -> Option<Vec<String>> {
        self.yt_tag
            .iter()
            .find(|ct| ct.channel == channel)
            .map(|ct| ct.tags.clone())
    }
    pub fn get_keyword_tags(&self, text: &str) -> Vec<String> {
        self.kw_tag
            .iter()
            .filter_map(|kt| {
                if text.to_lowercase().contains(&kt.keyword.to_lowercase()) {
                    Some(kt.tags.iter())
                } else {
                    None
                }
            })
            .flatten()
            .map(|t| t.to_string())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChannelTags {
    channel: String,
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeywordTags {
    keyword: String,
    tags: Vec<String>,
}
