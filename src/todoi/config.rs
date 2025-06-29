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
    fn keys_file() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt")
            .context("Failed to construct config path!")?;
        let keys_file = dirs.config_local_dir().join("keys.txt");
        Ok(keys_file)
    }
    pub fn parse() -> Result<Self> {
        let keys_file = Keys::keys_file()?;
        let text = std::fs::read_to_string(&keys_file)
            .context(format!("Could not read keys from {keys_file:?}"))?
            .replace("\r\n", "\n");
        toml::from_str(&text).context("Could not parse keys")
    }
}

pub struct Config {
    pub keys: Keys,
    tags: Tags,
}

impl Config {
    pub fn show_paths() {
        let tags_path = Tags::tags_config_path();
        let keys_file = Keys::keys_file().unwrap();

        println!("tags file: {tags_path:?}\nkeys file: {keys_file:?}");
    }

    pub fn load() -> Result<Self> {
        let keys = Keys::parse()?;
        let tags = Tags::parse()?;
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tags {
    yt_tag: Vec<ChannelTags>,
    kw_tag: Vec<KeywordTags>,
}

impl Tags {
    pub fn add_yt_tags(&mut self, channel: String, tags: Vec<String>) -> Result<()> {
        let yt_tag = self.yt_tag.iter_mut().find(|ct| ct.channel == channel);
        if let Some(yt_tag) = yt_tag {
            let tags_to_add: Vec<_> = tags
                .into_iter()
                .filter(|t| !yt_tag.tags.contains(t))
                .collect();
            tags_to_add.into_iter().for_each(|t| yt_tag.tags.push(t));
        } else {
            let ct = ChannelTags { channel, tags };
            self.yt_tag.push(ct);
        }
        self.write()
    }

    pub fn add_kw_tags(&mut self, kw: String, tags: Vec<String>) -> Result<()> {
        let kw_tag = self.kw_tag.iter_mut().find(|kwt| kwt.keyword == kw);
        if let Some(kw_tag) = kw_tag {
            let tags_to_add: Vec<_> = tags
                .into_iter()
                .filter(|t| !kw_tag.tags.contains(t))
                .collect();
            tags_to_add.into_iter().for_each(|t| kw_tag.tags.push(t));
        } else {
            let kwt = KeywordTags { keyword: kw, tags };
            self.kw_tag.push(kwt);
        }
        self.write()
    }

    pub fn parse() -> Result<Self> {
        let tags_path = Tags::tags_config_path();
        let text = std::fs::read_to_string(&tags_path)
            .context(format!("Failed to read tags file {tags_path:?}"))?
            .replace("\r\n", "\n");
        toml::from_str(&text).context("Failed to parse tags!")
    }

    fn write(&self) -> Result<()> {
        let tags_path = Tags::tags_config_path();
        let text =
            toml::to_string(self).context(format!("Failed to convert tags to string: {self:?}"))?;
        std::fs::write(&tags_path, text)
            .context(format!("Failed to write tags to {tags_path:?}"))?;
        Ok(())
    }

    fn tags_config_path() -> PathBuf {
        let dirs = directories::ProjectDirs::from("TF", "TF", "pkmt").unwrap();
        dirs.config_local_dir().join("todoi_tags.toml")
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
