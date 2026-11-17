//! Photos18 (`https://www.photos18.com`) — Rust port of the Keiyoushi
//! Tachiyomi extension `eu.kanade.tachiyomi.extension.all.photos18`.
//!
//! A Chinese photo-set gallery with a server-rendered card grid. Listing pages
//! live under the `/zh-hans` locale prefix while post pages also resolve
//! without it, so paths are stored without the prefix:
//!
//! | operation | endpoint / selectors |
//! |---|---|
//! | popular | `GET /zh-hans/sort/views?page={page}` — `#videos > div.card` |
//! | latest | `GET /zh-hans/?page={page}` — same grid |
//! | search | `GET /zh-hans/?q={query}&page={page}&sort=views` |
//! | details | `GET /v/{id}` — `h1`, `og:title`, `#content img` |
//! | chapters | single "Gallery" entry |
//! | pages | `GET /v/{id}` — `#content img[src]` |
//! | images | `img.photos18.com` — accepts direct requests |
//!
//! `MangaInfo.path` is the locale-less post path (`/v/rkbbv`).

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Photos18, NAME);

const ID: i64 = 225835804698740720;
const NAME: &str = "Photos18";
const URL: &str = "https://www.photos18.com";
const ICON_URL: &str = "https://www.photos18.com/logo.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Locale prefix used by the listing/search pages (simplified Chinese).
const LANG_PREFIX: &str = "/zh-hans";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct Photos18 {
    client: RateLimitedAgent,
}

impl Default for Photos18 {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
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

/// Strip the `/zh-hans` locale prefix from a link and make it root-relative.
fn locale_less_path(href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let mut path = href;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = href.strip_prefix(scheme) {
            path = match rest.find('/') {
                Some(index) => &rest[index..],
                None => "/",
            };
        }
    }
    let path = path.strip_prefix(LANG_PREFIX).unwrap_or(path);
    Some(if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    })
}

fn request_url(path: &str) -> String {
    let path = path.trim();
    if path.starts_with("https://") || path.starts_with("http://") {
        path.to_string()
    } else {
        format!("{URL}/{}", path.trim_start_matches('/'))
    }
}

impl Photos18 {
    fn fetch_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn parse_listing(&self, document: &Html) -> Vec<MangaInfo> {
        let card_selector = selector("#videos > div.card");
        let link_selector = selector(".card-body a");
        let label_selector = selector("label");
        let image_selector = selector("img");

        let mut mangas = Vec::new();
        for card in document.select(&card_selector) {
            let Some(link) = card.select(&link_selector).next() else {
                continue;
            };
            let title = link.text().collect::<String>().trim().to_string();
            let Some(path) = link.value().attr("href").and_then(locale_less_path) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let cover_url = card
                .select(&image_selector)
                .next()
                .and_then(|image| {
                    image
                        .value()
                        .attr("data-src")
                        .or_else(|| image.value().attr("src"))
                })
                .and_then(absolute)
                .unwrap_or_default();
            let genre = card
                .select(&label_selector)
                .next()
                .map(|label| label.text().collect::<String>().trim().to_string())
                .filter(|genre| !genre.is_empty())
                .into_iter()
                .collect();
            mangas.push(MangaInfo {
                source_id: ID,
                title,
                author: Vec::new(),
                genre,
                status: Some("Completed".to_string()),
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
                    bail!("{NAME}: no galleries found at {url} (markup may have changed)");
                }
                Ok(mangas)
            }
            Err(error) if page > 1 => {
                log::debug!("{NAME}: page {page} of {url} unavailable: {error:#}");
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn post_document(&self, path: &str) -> Result<Html> {
        self.fetch_page(&request_url(path))
    }

    fn content_images(&self, document: &Html) -> Vec<String> {
        let mut images = Vec::new();
        for image in document.select(&selector("#content img[src]")) {
            let Some(source) = image.value().attr("src") else {
                continue;
            };
            if let Some(url) = absolute(source)
                && !images.contains(&url)
            {
                images.push(url);
            }
        }
        images
    }
}

impl Extension for Photos18 {
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
        let page = page.max(1);
        let url = format!("{URL}{LANG_PREFIX}/sort/views?page={page}");
        self.listing(&url, page)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let url = format!("{URL}{LANG_PREFIX}/?page={page}");
        self.listing(&url, page)
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
            "{URL}{LANG_PREFIX}/?q={}&page={page}&sort=views",
            urlencoding::encode(query)
        );
        self.listing(&url, page)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let document = self.post_document(&path)?;
        let title = document
            .select(&selector("h1"))
            .next()
            .map(|element| element.text().collect::<String>().trim().to_string())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                document
                    .select(&selector("meta[property='og:title']"))
                    .next()
                    .and_then(|element| element.value().attr("content"))
                    .map(str::to_string)
            })
            .ok_or_else(|| anyhow::anyhow!("{NAME}: no title found at {path}"))?;

        let cover_url = self
            .content_images(&document)
            .first()
            .cloned()
            .unwrap_or_default();

        Ok(MangaInfo {
            source_id: ID,
            title,
            author: Vec::new(),
            genre: Vec::new(),
            status: Some("Completed".to_string()),
            description: None,
            path,
            cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        Ok(vec![ChapterInfo {
            source_id: ID,
            title: "Gallery".to_string(),
            path,
            number: 0.0,
            scanlator: None,
            uploaded: 0,
        }])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let document = self.post_document(&path)?;
        let images = self.content_images(&document);
        if images.is_empty() {
            bail!("{NAME}: no images found at {path} (markup may have changed)");
        }
        Ok(images)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        // The image CDN does not enforce a referer here, but send the site's
        // anyway to stay in line with the other ports.
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn strips_locale_prefix() {
        assert_eq!(
            locale_less_path("/zh-hans/v/rkbbv").as_deref(),
            Some("/v/rkbbv")
        );
        assert_eq!(
            locale_less_path("https://www.photos18.com/zh-hans/v/rqjK3").as_deref(),
            Some("/v/rqjK3")
        );
        assert_eq!(locale_less_path("/v/rqjK3").as_deref(), Some("/v/rqjK3"));
        assert_eq!(request_url("/v/rkbbv"), format!("{URL}/v/rkbbv"));
    }
}
