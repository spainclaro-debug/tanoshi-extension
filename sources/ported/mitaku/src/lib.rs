//! Mitaku (`https://mitaku.net`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.mitaku`.
//!
//! A WordPress cosplay/gravure photo blog. A post is a photo gallery rendered
//! in the theme's `msacwl` lightbox slider (or a video post with no gallery),
//! so each post maps to a single "Gallery" chapter:
//!
//! | operation | endpoint / selectors |
//! |---|---|
//! | popular | `GET /category/ero-cosplay/page/{page}` — `div.article-container article` |
//! | latest | `GET /page/{page}` — homepage/latest listing (Kotlin has no latest feed) |
//! | search | `GET /page/{page}?s={query}` — WordPress search results |
//! | details | `GET {post}` — `h1`, `.cat-links a`, `og:image`, `time.entry-date` |
//! | chapters | single "Gallery" entry (video-only posts are flagged) |
//! | pages | `GET {post}` — `a.msacwl-img-link[data-mfp-src]` |
//! | images | `mitaku.net/wp-content/uploads/...` — direct fetch works |
//!
//! `MangaInfo.path` keeps the site-relative post permalink.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, Mitaku, NAME);

const ID: i64 = 7541045796029140694;
const NAME: &str = "Mitaku";
const URL: &str = "https://mitaku.net";
const ICON_URL: &str = "https://mitaku.net/wp-content/uploads/2020/05/cropped-Mitaku-Logo-1-192x192.jpg";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct Mitaku {
    client: RateLimitedAgent,
}

impl Default for Mitaku {
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

fn request_url(path: &str) -> String {
    let path = path.trim();
    if path.starts_with("https://") || path.starts_with("http://") {
        path.to_string()
    } else {
        format!("{URL}/{}", path.trim_start_matches('/'))
    }
}

fn timestamp_millis(value: Option<&str>) -> i64 {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.timestamp_millis())
        .unwrap_or(0)
}

impl Mitaku {
    fn fetch_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn parse_listing(&self, document: &Html) -> Vec<MangaInfo> {
        let article_selector = selector("div.article-container article");
        let link_selector = selector("a");
        let title_selector = selector("h1, h2, h3");
        let image_selector = selector("img");

        let mut mangas = Vec::new();
        for article in document.select(&article_selector) {
            let Some(link) = article.select(&link_selector).next() else {
                continue;
            };
            let Some(path) = link.value().attr("href").and_then(site_path) else {
                continue;
            };
            let title = link
                .value()
                .attr("title")
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    article
                        .select(&title_selector)
                        .next()
                        .map(|element| element.text().collect::<String>().trim().to_string())
                        .filter(|title| !title.is_empty())
                });
            let Some(title) = title else {
                continue;
            };
            let cover_url = article
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
                log::debug!("{NAME}: page {page} of {url} unavailable: {error:#}");
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn post_document(&self, path: &str) -> Result<Html> {
        self.fetch_page(&request_url(path))
    }

    /// Gallery image URLs from the lightbox slider.
    fn gallery_images(&self, document: &Html) -> Vec<String> {
        let mut images = Vec::new();
        for link in document.select(&selector("a.msacwl-img-link")) {
            let source = link
                .value()
                .attr("data-mfp-src")
                .or_else(|| link.value().attr("href"))
                .map(str::trim)
                .unwrap_or_default();
            if source.is_empty() || source.starts_with("javascript:") {
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
}

impl Extension for Mitaku {
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
        let url = format!("{URL}/category/ero-cosplay/page/{page}");
        self.listing(&url, page)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        // Kotlin's source declares `supportsLatest = false`; the homepage is
        // the same chronological post stream, so use it as the latest feed.
        let page = page.max(1);
        self.listing(&format!("{URL}/page/{page}"), page)
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
            return self.get_popular_manga(page);
        }
        let url = format!("{URL}/page/{page}?s={}", urlencoding::encode(query));
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

        let mut genre = Vec::new();
        for link in document.select(&selector(".cat-links a")) {
            let name = link.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !genre.contains(&name) {
                genre.push(name);
            }
        }

        let cover_url = document
            .select(&selector("meta[property='og:image']"))
            .next()
            .and_then(|element| element.value().attr("content"))
            .and_then(absolute)
            .unwrap_or_default();

        Ok(MangaInfo {
            source_id: ID,
            title,
            author: Vec::new(),
            genre,
            status: Some("Completed".to_string()),
            description: None,
            path,
            cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let document = self.post_document(&path)?;
        let title = document
            .select(&selector("h1"))
            .next()
            .map(|element| element.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let uploaded = document
            .select(&selector("time.entry-date"))
            .next()
            .and_then(|element| element.value().attr("datetime"))
            .map(|value| timestamp_millis(Some(value)))
            .unwrap_or(0);

        let chapter_title = if title.ends_with("(Video)") {
            "This post is video-only, watch it in WebView".to_string()
        } else {
            "Gallery".to_string()
        };

        Ok(vec![ChapterInfo {
            source_id: ID,
            title: chapter_title,
            path,
            number: 1.0,
            scanlator: None,
            uploaded,
        }])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let document = self.post_document(&path)?;
        let images = self.gallery_images(&document);
        if images.is_empty() {
            bail!("{NAME}: page list not found at {path} (video-only post or markup changed)");
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
            site_path("https://mitaku.net/ero-cosplay/aiko/").as_deref(),
            Some("/ero-cosplay/aiko/")
        );
        assert_eq!(site_path("ero-cosplay/aiko/").as_deref(), Some("/ero-cosplay/aiko/"));
        assert_eq!(
            request_url("/ero-cosplay/aiko/"),
            format!("{URL}/ero-cosplay/aiko/")
        );
    }

    #[test]
    fn absolutises_images() {
        assert_eq!(
            absolute("/wp-content/uploads/a.jpg").as_deref(),
            Some("https://mitaku.net/wp-content/uploads/a.jpg")
        );
        assert_eq!(absolute("data:image/png;base64,x"), None);
    }
}
