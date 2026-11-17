//! OSOSEDKI (`https://ososedki.com`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.ososedki`.
//!
//! The site exposes an HTML-fragment JSON API for listings and server-rendered
//! album pages for the rest:
//!
//! | operation | request |
//! |---|---|
//! | popular | `GET /api/albums?page={page}&type=top&value=1` |
//! | latest | `GET /api/albums?page={page}` |
//! | search | `GET /api/albums?page={page}&type=search&value={query}` |
//! | details | `GET /photos/{albumId}` — `h1`, `a[href^=/model/]`, `og:image` |
//! | chapters | `GET /photos/{albumId}` — single "Gallery"; date from ld+json |
//! | pages | `GET /photos/{albumId}` — `#photos a[href^=/images/]` |
//! | images | `GET /images/a/1280/...` with fallback to the `604` rendition |
//!
//! Album ids have the form `{ownerId}_{postId}` (owner id may be negative) and
//! are used directly as `MangaInfo.path` / `ChapterInfo.path`. Listing thumbnails
//! come from `/images/albums/{ownerId}/{postId}.webp`.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Ososedki, NAME);

const ID: i64 = 703138310497011533;
const NAME: &str = "OSOSEDKI";
const URL: &str = "https://ososedki.com";
const ICON_URL: &str = "https://ososedki.com/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct Ososedki {
    client: RateLimitedAgent,
}

impl Default for Ososedki {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AlbumsResponse {
    #[serde(default)]
    html: String,
    #[serde(default, rename = "hasMore")]
    has_more: bool,
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static CSS selector is valid")
}

fn absolute(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("data:") {
        return None;
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url.to_string());
    }
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    Some(format!("{URL}/{}", url.trim_start_matches('/')))
}

/// Valid album id: `[{'-'}]digits '_' digits`, e.g. `-10000001_10016685`.
fn valid_album_id(id: &str) -> bool {
    let Some((owner, post)) = id.split_once('_') else {
        return false;
    };
    let owner = owner.strip_prefix('-').unwrap_or(owner);
    !owner.is_empty()
        && !post.is_empty()
        && owner.chars().all(|c| c.is_ascii_digit())
        && post.chars().all(|c| c.is_ascii_digit())
}

/// Pull the album id out of a `/photos/{albumId}` link or stored path.
fn album_id(value: &str) -> Option<String> {
    let value = value.trim();
    if valid_album_id(value) {
        return Some(value.to_string());
    }
    let mut path = value;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = value.strip_prefix(scheme) {
            path = match rest.find('/') {
                Some(index) => &rest[index..],
                None => "/",
            };
        }
    }
    let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
    let photos = segments.iter().position(|segment| *segment == "photos")?;
    let id = segments.get(photos + 1)?;
    valid_album_id(id).then(|| id.to_string())
}

/// Site-relative `/photos/{albumId}` path.
fn photo_path(id: &str) -> String {
    format!("/photos/{id}")
}

fn photo_url(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") {
        value.to_string()
    } else if let Some(id) = album_id(value) {
        format!("{URL}{}", photo_path(&id))
    } else {
        format!("{URL}/{}", value.trim_start_matches('/'))
    }
}

/// Cover URL derived from the album id (`ownerId_postId`).
fn cover_from_album_id(id: &str) -> Option<String> {
    let (owner, post) = id.split_once('_')?;
    Some(format!("{URL}/images/albums/{owner}/{post}.webp"))
}

/// Drop the "(NN leaked photos) from Onlyfans, ..." suffix from the page title.
fn strip_title_suffix(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    if let Some(index) = lower.find(" leaked photos)")
        && let Some(open) = title[..index].rfind(" (")
    {
        let stripped = title[..open].trim();
        if !stripped.is_empty() {
            return stripped.to_string();
        }
    }
    title.trim().to_string()
}

fn timestamp_from_ld_json(document: &Html) -> i64 {
    for script in document.select(&selector("script[type='application/ld+json']")) {
        let text = script.text().collect::<String>();
        let marker = "\"datePublished\":\"";
        if let Some(start) = text.find(marker) {
            let rest = &text[start + marker.len()..];
            if let Some(end) = rest.find('"') {
                return chrono::DateTime::parse_from_rfc3339(&rest[..end])
                    .map(|date| date.timestamp_millis())
                    .unwrap_or(0);
            }
        }
    }
    0
}

impl Ososedki {
    fn fetch_text(&self, url: &str) -> Result<String> {
        log::debug!("{NAME}: GET {url}");
        self.client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))
    }

    fn fetch_page(&self, url: &str) -> Result<Html> {
        Ok(Html::parse_document(&self.fetch_text(url)?))
    }

    fn albums_url(page: i64, kind: Option<(&str, &str)>) -> String {
        let mut url = format!("{URL}/api/albums?page={}", page.max(1));
        if let Some((kind, value)) = kind {
            url.push_str(&format!(
                "&type={}&value={}",
                urlencoding::encode(kind),
                urlencoding::encode(value)
            ));
        }
        url
    }

    fn fetch_albums(&self, page: i64, kind: Option<(&str, &str)>) -> Result<Vec<MangaInfo>> {
        let url = Self::albums_url(page, kind);
        let response: AlbumsResponse = serde_json::from_str(&self.fetch_text(&url)?)
            .with_context(|| format!("decoding albums JSON from {url}"))?;

        let document = Html::parse_fragment(&response.html);
        let entries = self.parse_entries(&document);
        if entries.is_empty() && page <= 1 {
            bail!(
                "{NAME}: no albums found for {url} (hasMore={})",
                response.has_more
            );
        }
        Ok(entries)
    }

    fn parse_entries(&self, document: &Html) -> Vec<MangaInfo> {
        let entry_selector = selector("article.gallery-item");
        let link_selector = selector("a.gallery-link");
        let title_selector = selector("h3");
        let image_selector = selector("img.gallery-img");
        let author_selector = selector("span.badge:not(.bg-dark)");

        let mut mangas = Vec::new();
        for entry in document.select(&entry_selector) {
            let link = match entry.select(&link_selector).next() {
                Some(link) => link,
                None => continue,
            };
            let Some(id) = link.value().attr("href").and_then(album_id) else {
                continue;
            };
            let title = entry
                .select(&title_selector)
                .next()
                .map(|element| element.text().collect::<String>().trim().to_string())
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    entry
                        .select(&image_selector)
                        .next()
                        .and_then(|image| image.value().attr("alt"))
                        .map(|alt| alt.split(" nude.").next().unwrap_or(alt).trim().to_string())
                        .filter(|title| !title.is_empty())
                });
            let Some(title) = title else {
                continue;
            };
            let cover_url = entry
                .select(&image_selector)
                .next()
                .and_then(|image| {
                    image
                        .value()
                        .attr("data-src")
                        .or_else(|| image.value().attr("src"))
                })
                .and_then(absolute)
                .or_else(|| cover_from_album_id(&id))
                .unwrap_or_default();
            let author = entry
                .select(&author_selector)
                .next()
                .map(|element| element.text().collect::<String>().trim().to_string())
                .filter(|author| !author.is_empty())
                .into_iter()
                .collect();
            mangas.push(MangaInfo {
                source_id: ID,
                title,
                author,
                genre: Vec::new(),
                status: None,
                description: None,
                path: id,
                cover_url,
            });
        }
        mangas
    }

    fn tag_texts(&self, document: &Html, prefix: &str) -> Vec<String> {
        let css = format!("a[href^='{prefix}']");
        let mut tags = Vec::new();
        if let Ok(selector) = Selector::parse(&css) {
            for link in document.select(&selector) {
                let text = link.text().collect::<String>().trim().to_string();
                if !text.is_empty() && !tags.contains(&text) {
                    tags.push(text);
                }
            }
        }
        tags
    }
}

impl Extension for Ososedki {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::All,
            nsfw: true,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.fetch_albums(page, Some(("top", "1")))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.fetch_albums(page, None)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let query = query.trim();
        if query.is_empty() {
            return self.get_popular_manga(page);
        }
        self.fetch_albums(page, Some(("search", query)))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let id = album_id(&path).ok_or_else(|| {
            anyhow::anyhow!("{NAME}: invalid album path {path:?} (expected ownerId_postId)")
        })?;
        let url = photo_url(&id);
        let document = self.fetch_page(&url)?;

        let models = self.tag_texts(&document, "/model/");
        let cosplays = self.tag_texts(&document, "/cosplay/");
        let fandoms = self.tag_texts(&document, "/fandom/");

        let title = [models.first(), cosplays.first(), fandoms.first()]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(" - ");
        let title = if title.is_empty() {
            document
                .select(&selector("h1"))
                .next()
                .map(|element| element.text().collect::<String>())
                .map(|title| strip_title_suffix(&title))
                .unwrap_or_default()
        } else {
            title
        };
        if title.is_empty() {
            bail!("{NAME}: title is missing for album {id}");
        }

        let cover_url = cover_from_album_id(&id)
            .or_else(|| {
                document
                    .select(&selector("meta[property='og:image']"))
                    .next()
                    .and_then(|element| element.value().attr("content"))
                    .and_then(absolute)
            })
            .unwrap_or_default();

        let mut genre = Vec::new();
        for tag in models.iter().chain(cosplays.iter()).chain(fandoms.iter()) {
            if !genre.contains(tag) {
                genre.push(tag.clone());
            }
        }

        Ok(MangaInfo {
            source_id: ID,
            title,
            author: models,
            genre,
            status: Some("Completed".to_string()),
            description: None,
            path: id,
            cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let id = album_id(&path).ok_or_else(|| {
            anyhow::anyhow!("{NAME}: invalid album path {path:?} (expected ownerId_postId)")
        })?;
        let document = self.fetch_page(&photo_url(&id))?;
        let uploaded = timestamp_from_ld_json(&document);

        Ok(vec![ChapterInfo {
            source_id: ID,
            title: "Gallery".to_string(),
            path: id,
            number: 0.0,
            scanlator: None,
            uploaded,
        }])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let id = album_id(&path).ok_or_else(|| {
            anyhow::anyhow!("{NAME}: invalid album path {path:?} (expected ownerId_postId)")
        })?;
        let url = photo_url(&id);
        let document = self.fetch_page(&url)?;

        let mut images: Vec<String> = Vec::new();
        for link in document.select(&selector("#photos a[href]")) {
            let Some(href) = link.value().attr("href") else {
                continue;
            };
            if !href.starts_with("/images/") && !href.starts_with("https://ososedki.com/images/") {
                continue;
            }
            if let Some(image) = absolute(href)
                && !images.contains(&image)
            {
                images.push(image);
            }
        }
        images.sort_by_key(|url| page_number(url));
        if images.is_empty() {
            bail!("{NAME}: no images found for album {id} at {url}");
        }
        Ok(images)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        log::debug!("{NAME}: get_image_bytes url={url}");
        match self.client.fetch_bytes(&url, Some(URL)) {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                // The site occasionally only stores the smaller rendition; the
                // Kotlin source falls back from `/images/a/1280/` to `604`.
                let fallback = url.replace("/images/a/1280/", "/images/a/604/");
                if fallback == url {
                    return Err(error).with_context(|| format!("requesting {url}"));
                }
                log::debug!("{NAME}: falling back to {fallback} after {error:#}");
                self.client
                    .fetch_bytes(&fallback, Some(URL))
                    .with_context(|| format!("requesting {fallback}"))
            }
        }
    }
}

/// Numeric page number from the final path segment (`12.webp` -> 12).
fn page_number(url: &str) -> i64 {
    url.rsplit('/')
        .next()
        .and_then(|segment| segment.split('.').next())
        .and_then(|number| number.parse::<i64>().ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_album_ids() {
        assert_eq!(album_id("/photos/-10000001_10016685").as_deref(), Some("-10000001_10016685"));
        assert_eq!(
            album_id("https://ososedki.com/photos/123_456/").as_deref(),
            Some("123_456")
        );
        assert_eq!(album_id("-10000001_10016685").as_deref(), Some("-10000001_10016685"));
        assert_eq!(album_id("/photos/not-an-id"), None);
        assert_eq!(album_id("/model/King+Angel"), None);
    }

    #[test]
    fn strips_onlyfans_title_suffix() {
        assert_eq!(
            strip_title_suffix(
                "King Angel - Tifa Lockhart (74 leaked photos) from Onlyfans, Patreon and Fansly"
            ),
            "King Angel - Tifa Lockhart"
        );
        assert_eq!(strip_title_suffix("Plain title"), "Plain title");
    }

    #[test]
    fn builds_covers_and_sorts_pages() {
        assert_eq!(
            cover_from_album_id("-10000001_10016685").as_deref(),
            Some("https://ososedki.com/images/albums/-10000001/10016685.webp")
        );
        let mut urls = vec![
            "https://ososedki.com/images/a/1280/-1/2/10.webp".to_string(),
            "https://ososedki.com/images/a/1280/-1/2/2.webp".to_string(),
            "https://ososedki.com/images/a/1280/-1/2/1.webp".to_string(),
        ];
        urls.sort_by_key(|url| page_number(url));
        assert_eq!(
            urls,
            vec![
                "https://ososedki.com/images/a/1280/-1/2/1.webp",
                "https://ososedki.com/images/a/1280/-1/2/2.webp",
                "https://ososedki.com/images/a/1280/-1/2/10.webp",
            ]
        );
    }
}
