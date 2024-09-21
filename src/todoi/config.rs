use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Config {
    yt_tag: Vec<ChannelTags>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ChannelTags {
    channel: String,
    tags: Vec<String>,
}
