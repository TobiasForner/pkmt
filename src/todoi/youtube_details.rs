use anyhow::{bail, Result};
use std::str::FromStr;

pub fn youtube_details(video_url: &str, api_key: &str) -> Result<(String, String)> {
    let client = reqwest::Client::new();
    let video_url = reqwest::Url::from_str(video_url)?;
    if let Some((_, id)) = video_url.query_pairs().find(|(k, _)| k == "v") {
        let res = client
            .get("https://www.googleapis.com/youtube/v3/videos")
            .query(&[("key", api_key), ("part", "snippet"), ("id", &id)])
            .send();

        let runtime = tokio::runtime::Runtime::new()?;
        let res = runtime.block_on(res);

        let text = runtime.block_on(res?.text())?;
        let mut js = json::parse(&text)?;
        let snippet = js["items"].pop()["snippet"].clone();
        let title = snippet["title"].to_string();
        let channel = snippet["channelTitle"].to_string();

        Ok((title, channel))
    } else {
        bail!("Could not extract url from {video_url}!");
    }
}
/// returns (description, channel)
pub fn youtube_playlist_details(playlist_url: &str, api_key: &str) -> Result<(String, String)> {
    let client = reqwest::Client::new();
    let playlist_url = reqwest::Url::from_str(playlist_url)?;
    if let Some((_, id)) = playlist_url.query_pairs().find(|(k, _)| k == "list") {
        let res = client
            .get("https://www.googleapis.com/youtube/v3/playlists")
            .query(&[("key", api_key), ("part", "snippet"), ("id", &id)])
            .send();
        let runtime = tokio::runtime::Runtime::new()?;
        let res = runtime.block_on(res);

        let text = runtime.block_on(res?.text())?;
        let mut js = json::parse(&text)?;
        let snippet = js["items"].pop()["snippet"].clone();
        let title = snippet["title"].to_string();
        let channel = snippet["channelTitle"].to_string();
        let description = snippet["description"].to_string().replace("\n", " ");

        return Ok((format!("{title}: {description}"), channel));
    }
    bail!("Could not extract details from playlist url {playlist_url}!")
}
