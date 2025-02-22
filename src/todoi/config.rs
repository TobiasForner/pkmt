use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Keys {
    pub yt_api_key: String,
    pub todoist_api_key: String,
}

impl Keys {
    pub fn parse() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt").unwrap();
        let keys_file = dirs.config_local_dir().join("keys.txt");
        let text = std::fs::read_to_string(&keys_file)
            .context(format!("Could not read keys from {keys_file:?}"))?
            .replace("\r\n", "\n");
        toml::from_str(&text).context("Could not parse keys")
    }
}

pub struct Config {
    //pub yt_api_key: String,
    //pub todoist_api_key: String,
    pub keys: Keys,
    tags: Tags,
}

impl Config {
    pub fn load(tags_path: &PathBuf) -> Result<Self> {
        let keys = Keys::parse()?;
        let tags = Tags::parse(tags_path)?;
        Ok(Config { keys, tags })
    }

    pub fn get_channel_tags(&self, channel: &str) -> Option<Vec<String>> {
        self.tags
            .yt_tag
            .iter()
            .find(|ct| ct.channel == channel)
            .map(|ct| ct.tags.clone())
    }
    pub fn get_keyword_tags(&self, text: &str) -> Vec<String> {
        self.tags
            .kw_tag
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

#[derive(Debug, Clone, Deserialize)]
struct Tags {
    yt_tag: Vec<ChannelTags>,
    kw_tag: Vec<KeywordTags>,
}

impl Tags {
    fn parse(tags_path: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(tags_path)?.replace("\r\n", "\n");
        toml::from_str(&text).context("Failed to parse tags!")
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
