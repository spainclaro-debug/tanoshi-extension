//! MissKon (`https://misskon.com`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.misskon`.
//!
//! A WordPress photo-blog. Gallery posts are paginated by the theme's
//! `<!--nextpage-->` feature, so the Kotlin source treats each *page of the
//! post* as a "chapter" and the images on that page as its pages:
//!
//! | operation | endpoint / selectors |
//! |---|---|
//! | popular | `GET /top3/` — `article.item-list` (fixed, page 2 is empty) |
//! | latest | `GET /page/{page}/` — `article.item-list` |
//! | search | `GET /page/{page}/?s={query}` — same grid |
//! | details | `GET {post}` — `.post-title`, `.post-tag a`, `.page-link` |
//! | chapters | post pages 1..N (`Page N`), date from `/uploads/yyyy/mm/dd/` |
//! | pages | `GET {post}/{n}/` — `div.entry img[data-src]` |
//! | images | `kul.misskon.com` — accepts direct requests, same-site referer sent |
//!
//! `MangaInfo.path` keeps the site-relative post permalink (with trailing
//! slash) and `ChapterInfo.path` is the paginated post URL.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, MissKon, NAME);

const ID: i64 = 492049561920540214;
const NAME: &str = "MissKon";
const URL: &str = "https://misskon.com";
const ICON_URL: &str = "https://misskon.com/media/2023/12/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The Kotlin source configures `rateLimit(10, 1.seconds)` for this host.
const REQUESTS_PER_SECOND: f64 = 10.0;
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct MissKon {
    client: RateLimitedAgent,
}

impl Default for MissKon {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), Some(REQUESTS_PER_SECOND)),
        }
    }
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static CSS selector is valid")
}

/// Turn a relative/absolute image URL into an absolute one.
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

/// Reduce an absolute or root-relative site link to a site-relative path.
fn site_path(href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    for scheme in ["https://", "http://"] {
        if let Some(rest) = href.strip_prefix(scheme) {
            let path = match rest.find('/') {
                Some(index) => &rest[index..],
                None => "/",
            };
            return Some(path.to_string());
        }
    }
    Some(if href.starts_with('/') {
        href.to_string()
    } else {
        format!("/{href}")
    })
}

/// Normalise a stored manga/chapter path to a full request URL.
fn request_url(path: &str) -> String {
    let path = path.trim();
    if path.starts_with("https://") || path.starts_with("http://") {
        path.to_string()
    } else {
        format!("{URL}/{}", path.trim_start_matches('/'))
    }
}

/// Path of one page of a paginated post. Page 1 is the post itself.
fn chapter_page_path(manga_path: &str, page: i64) -> String {
    let base = manga_path.trim().trim_end_matches('/');
    if page <= 1 {
        format!("{base}/")
    } else {
        format!("{base}/{page}/")
    }
}

/// Date encoded in a MissKon upload path (`/uploads/2026/11/14/...`).
fn date_from_upload_url(url: &str) -> i64 {
    let segments: Vec<&str> = url.split('/').collect();
    for window in segments.windows(3) {
        let (year, month) = (window[0], window[1]);
        if year.len() != 4 || !year.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(year) = year.parse::<i32>() else {
            continue;
        };
        let Ok(month) = month.parse::<u32>() else {
            continue;
        };
        if !(1..=12).contains(&month) {
            continue;
        }
        let day = window[2]
            .parse::<u32>()
            .ok()
            .filter(|day| (1..=31).contains(day))
            .unwrap_or(1);
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
        {
            return date.and_utc().timestamp_millis();
        }
    }
    0
}

/// Gallery images on the post body: lazy-loaded `data-src` images inside the
/// first `div.entry`. Related-post thumbnails live outside that container and
/// advertising images are filtered by class/host.
fn gallery_images(document: &Html) -> Vec<String> {
    let entry_selector = selector("div.entry");
    let image_selector = selector("img");
    let mut images = Vec::new();
    let Some(entry) = document.select(&entry_selector).next() else {
        return images;
    };
    for image in entry.select(&image_selector) {
        let class = image.value().attr("class").unwrap_or_default();
        if class.contains("attachment") {
            continue;
        }
        let Some(source) = image
            .value()
            .attr("data-src")
            .or_else(|| image.value().attr("src"))
        else {
            continue;
        };
        if source.starts_with("data:") {
            continue;
        }
        if let Some(url) = absolute(source)
            && !images.contains(&url)
        {
            images.push(url);
        }
    }
    images
}

struct Details {
    title: String,
    genres: Vec<String>,
    cover_url: String,
    max_page: i64,
    uploaded: i64,
}

fn parse_details(document: &Html) -> Option<Details> {
    let title = document
        .select(&selector(".post-title"))
        .next()
        .or_else(|| document.select(&selector("h1")).next())
        .map(|element| element.text().collect::<String>().trim().to_string())
        .filter(|title| !title.is_empty())?;

    let mut genres: Vec<String> = Vec::new();
    for link in document.select(&selector(".post-tag a")) {
        let genre = link.text().collect::<String>().trim().to_string();
        if !genre.is_empty() && !genres.contains(&genre) {
            genres.push(genre);
        }
    }

    let images = gallery_images(document);
    let uploaded = images
        .first()
        .map(|image| date_from_upload_url(image))
        .unwrap_or(0);

    let max_page = document
        .select(&selector(".page-link"))
        .next()
        .and_then(|element| {
            element
                .select(&selector("a.post-page-numbers"))
                .last()
                .and_then(|link| link.text().collect::<String>().trim().parse::<i64>().ok())
        })
        .unwrap_or(1)
        .max(1);

    Some(Details {
        title,
        genres,
        cover_url: images.first().cloned().unwrap_or_default(),
        max_page,
        uploaded,
    })
}

impl MissKon {
    fn fetch_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn parse_listing(&self, document: &Html) -> Vec<MangaInfo> {
        let article_selector = selector("article.item-list");
        let link_selector = selector(".post-box-title a");
        let thumb_selector = selector(".post-thumbnail img");

        let mut mangas = Vec::new();
        for article in document.select(&article_selector) {
            let Some(link) = article.select(&link_selector).next() else {
                continue;
            };
            let title = link.text().collect::<String>().trim().to_string();
            let Some(path) = link.value().attr("href").and_then(site_path) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let cover_url = article
                .select(&thumb_selector)
                .next()
                .and_then(|image| {
                    image
                        .value()
                        .attr("data-src")
                        .or_else(|| image.value().attr("src"))
                })
                .and_then(absolute)
                .unwrap_or_default();
            mangas.push(MangaInfo {
                source_id: ID,
                title,
                author: Vec::new(),
                genre: Vec::new(),
                status: None,
                description: None,
                path,
                cover_url,
            });
        }
        mangas
    }

    fn listing(&self, url: &str, page: i64) -> Result<Vec<MangaInfo>> {
        match self.fetch_page(url) {
            Ok(document) => {
                let mangas = self.parse_listing(&document);
                if mangas.is_empty() && page <= 1 {
                    bail!("{NAME}: no posts found at {url} (markup may have changed)");
                }
                Ok(mangas)
            }
            Err(error) if page > 1 => {
                // End of pagination: the site answers 404 for pages past the
                // last one. Surface the real error on page 1 only.
                log::debug!("{NAME}: page {page} of {url} unavailable: {error:#}");
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn detail_document(&self, path: &str) -> Result<Html> {
        self.fetch_page(&request_url(path))
    }
}

impl Extension for MissKon {
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
        // The Kotlin source always shows the fixed Top-3-days feed and reports
        // no next page.
        if page > 1 {
            return Ok(Vec::new());
        }
        self.listing(&format!("{URL}/top3/"), page)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        self.listing(&format!("{URL}/page/{page}/"), page)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let query = query.unwrap_or_default();
        let query = query.trim();
        if query.is_empty() {
            return self.get_latest_manga(page);
        }
        let url = format!(
            "{URL}/page/{page}/?s={}",
            urlencoding::encode(query)
        );
        self.listing(&url, page)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let document = self.detail_document(&path)?;
        let details = parse_details(&document)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: no post title found at {path}"))?;
        Ok(MangaInfo {
            source_id: ID,
            title: details.title,
            author: Vec::new(),
            genre: details.genres,
            status: Some("Completed".to_string()),
            description: None,
            path,
            cover_url: details.cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let document = self.detail_document(&path)?;
        let details = parse_details(&document)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: no post found at {path}"))?;
        if details.max_page <= 0 {
            bail!("{NAME}: no gallery pages found at {path}");
        }
        let chapters = (1..=details.max_page)
            .rev()
            .map(|page| ChapterInfo {
                source_id: ID,
                title: format!("Page {page}"),
                path: chapter_page_path(&path, page),
                number: page as f64,
                scanlator: None,
                uploaded: details.uploaded,
            })
            .collect();
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = request_url(&path);
        let document = self.fetch_page(&url)?;
        let images = gallery_images(&document);
        if images.is_empty() {
            bail!("{NAME}: no images found at {url} (markup may have changed)");
        }
        Ok(images)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn normalises_site_paths() {
        assert_eq!(
            site_path("https://misskon.com/123-post/").as_deref(),
            Some("/123-post/")
        );
        assert_eq!(site_path("/123-post/").as_deref(), Some("/123-post/"));
        assert_eq!(site_path("123-post/").as_deref(), Some("/123-post/"));
        assert_eq!(request_url("/123-post/"), format!("{URL}/123-post/"));
    }

    #[test]
    fn builds_chapter_page_paths() {
        assert_eq!(chapter_page_path("/123-post/", 1), "/123-post/");
        assert_eq!(chapter_page_path("/123-post/", 3), "/123-post/3/");
        assert_eq!(chapter_page_path("https://misskon.com/123/", 2), "https://misskon.com/123/2/");
    }

    #[test]
    fn parses_upload_dates() {
        assert_eq!(
            date_from_upload_url("https://kul.misskon.com/uploads/2026/11/14/photo.webp"),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 14)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis()
        );
        assert_eq!(
            date_from_upload_url("https://kul.misskon.com/uploads/2026/11/photo.webp"),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis()
        );
        assert_eq!(date_from_upload_url("https://misskon.com/img/logo.png"), 0);
    }
}
