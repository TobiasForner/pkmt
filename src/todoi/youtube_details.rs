use anyhow::{Result, bail};
use std::str::FromStr;

/// returns (title, channel)
pub fn youtube_details(video_url: &str, api_key: &str) -> Result<(String, String)> {
    let client = reqwest::Client::new();
    let resolved = client.get(video_url).send();
    let runtime = tokio::runtime::Runtime::new()?;
    let res = runtime.block_on(resolved);

    let video_url = if let Ok(res) = res {
        res.url().to_string()
    } else {
        video_url.to_string()
    };
    println!("Resolved {video_url} to {video_url}");
    let id = if let Some(pos) = video_url.find("/shorts/") {
        Some(video_url[pos + 8..video_url.len()].to_string())
    } else {
        reqwest::Url::from_str(&video_url)?
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, id)| id.to_string())
    };
    println!("{video_url}-> {id:?}");
    if let Some(id) = id {
        let res = client
            .get("https://www.googleapis.com/youtube/v3/videos")
            .query(&[("key", api_key), ("part", "snippet"), ("id", &id)])
            .send();

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

#[test]
fn get_yt_details() {
    use crate::todoi::config::Config;
    let config = Config::load().unwrap();
    let api_key = &config.keys.yt_api_key;
    let yt_url = "https://www.youtube.com/watch?v=NkM6wQL2UvM";
    let details = youtube_details(yt_url, api_key).unwrap();
    assert_eq!(
        details.0,
        "The Kubernetes Homelab That Prints Job Offers (Simple & Proven)"
    );
    assert_eq!(details.1, "Mischa van den Burg");
}
