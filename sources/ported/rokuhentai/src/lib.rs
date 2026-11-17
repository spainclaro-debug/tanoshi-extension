//! RokuHentai (`https://rokuhentai.com`) — Rust port of the Keiyoushi
//! Tachiyomi extension `eu.kanade.tachiyomi.extension.all.rokuhentai`.
//!
//! A doujinshi reader with a Material-card listing. The homepage is a
//! server-rendered card grid; additional pages are loaded from `/_search`,
//! which answers JSON whose `manga-cards` field holds HTML fragments:
//!
//! | operation | request |
//! |---|---|
//! | popular / latest | `GET /` then `GET /_search?p={lastCardId}` chains |
//! | search | `GET /?q={query}` then `GET /_search?q={query}&p={lastCardId}` |
//! | details | `GET /{id}` — `.site-manga-info` cover, title and chips |
//! | chapters | single gallery; the reader page embeds every page URL |
//! | pages | `GET /{id}/0` — `img.site-reader__image[data-src]` |
//! | images | `/_images/pages/{id}/{n}.jpg` — 403 without a site Referer |
//!
//! The Kotlin source has no latest feed (`supportsLatest = false`); the
//! homepage is the chronological newest-first grid, so it serves as both
//! popular and latest here. `MangaInfo.path` is the short gallery id.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, RokuHentai, NAME);

const ID: i64 = 6773816792833894188;
const NAME: &str = "RokuHentai";
const URL: &str = "https://rokuhentai.com";
const ICON_URL: &str = "https://rokuhentai.com/logo-192x192.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Pagination is a chain of `_search?p=` requests; cap it like a cursor walk.
const MAX_LIST_PAGES: i64 = 20;
const REFERER: &str = "https://rokuhentai.com/";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

pub struct RokuHentai {
    client: RateLimitedAgent,
}

impl Default for RokuHentai {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct SearchResult {
    #[serde(default, rename = "manga-cards")]
    manga_cards: Vec<String>,
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static CSS selector is valid")
}

/// Extract the URL out of a `background-image: url("...")` style attribute.
fn background_image_url(style: &str) -> Option<String> {
    let cleaned = style.replace(['"', '\''], "");
    let start = cleaned.find("url(")? + 4;
    let rest = &cleaned[start..];
    let end = rest.find(')')?;
    let url = rest[..end].trim();
    (!url.is_empty()).then(|| url.to_string())
}

/// Gallery id from a reader/details link (`/2aj8li/0` -> `2aj8li`).
fn gallery_id(href: &str) -> Option<String> {
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
    path.split('/')
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

fn request_url(path: &str) -> String {
    let path = path.trim();
    if path.starts_with("https://") || path.starts_with("http://") {
        path.to_string()
    } else {
        format!("{URL}/{}", path.trim_start_matches('/'))
    }
}

/// Reader path for one gallery id: `/{id}/0`.
fn reader_path(id: &str) -> String {
    format!("/{id}/0")
}

/// Parse "Nov 17, 2026,  6:16 pm" (UTC) into unix millis.
fn parse_caption_date(caption: &str) -> i64 {
    let normalized = caption.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some((_, date_part)) = normalized.split_once(" images ") else {
        return 0;
    };
    let formats = ["%b %d, %Y, %I:%M %p", "%b %d, %Y, %H:%M"];
    for format in formats {
        if let Ok(date) = chrono::NaiveDateTime::parse_from_str(date_part.trim(), format) {
            return date.and_utc().timestamp_millis();
        }
    }
    0
}

struct Details {
    title: String,
    description: Option<String>,
    cover_url: String,
    genres: Vec<String>,
    authors: Vec<String>,
    uploaded: i64,
}

fn parse_details(document: &Html) -> Option<Details> {
    let title = document
        .select(&selector(".site-manga-info h6"))
        .next()
        .map(|element| element.text().collect::<String>().trim().to_string())
        .filter(|title| !title.is_empty())?;

    let caption = document
        .select(&selector(".site-manga-info .mdc-typography--caption"))
        .next()
        .map(|element| element.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|caption| !caption.is_empty());
    let uploaded = caption
        .as_deref()
        .map(parse_caption_date)
        .unwrap_or(0);
    let description = caption;

    let cover_url = document
        .select(&selector(".site-manga-info .mdc-card__media"))
        .next()
        .and_then(|element| element.value().attr("style"))
        .and_then(background_image_url)
        .unwrap_or_default();

    let mut genres = Vec::new();
    let mut authors = Vec::new();
    for chip in document.select(&selector(".site-manga-info .mdc-chip__text")) {
        let text = chip.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        if let Some(artist) = text.strip_prefix("artist:") {
            let artist = artist.trim().to_string();
            if !artist.is_empty() && !authors.contains(&artist) {
                authors.push(artist);
            }
        }
        if !genres.contains(&text) {
            genres.push(text);
        }
    }

    Some(Details {
        title,
        description,
        cover_url,
        genres,
        authors,
        uploaded,
    })
}

impl RokuHentai {
    fn fetch_text(&self, url: &str) -> Result<String> {
        log::debug!("{NAME}: GET {url}");
        self.client
            .fetch_text_with_headers(url, &[("Referer", REFERER)])
            .with_context(|| format!("requesting {url}"))
    }

    fn fetch_page(&self, url: &str) -> Result<Html> {
        Ok(Html::parse_document(&self.fetch_text(url)?))
    }

    fn parse_cards(&self, document: &Html) -> Vec<MangaInfo> {
        let card_selector = selector("div.mdc-card");
        let link_selector = selector("a.site-popunder-ad-slot[href]");
        let title_selector = selector(".site-manga-card__title--primary");
        let media_selector = selector(".mdc-card__media");

        let mut mangas: Vec<MangaInfo> = Vec::new();
        for card in document.select(&card_selector) {
            let Some(link) = card.select(&link_selector).next() else {
                continue;
            };
            let Some(id) = link.value().attr("href").and_then(gallery_id) else {
                continue;
            };
            let title = card
                .select(&title_selector)
                .next()
                .map(|element| element.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            if mangas.iter().any(|manga| manga.path == id) {
                continue;
            }
            let cover_url = card
                .select(&media_selector)
                .next()
                .and_then(|element| element.value().attr("style"))
                .and_then(background_image_url)
                .unwrap_or_default();
            mangas.push(MangaInfo {
                source_id: ID,
                title,
                author: Vec::new(),
                genre: Vec::new(),
                status: Some("Completed".to_string()),
                description: None,
                path: id,
                cover_url,
            });
        }
        mangas
    }

    /// Cards from one raw response: HTML listing or `_search` JSON fragments.
    fn parse_cards_from_body(&self, body: &str) -> Vec<MangaInfo> {
        if body.trim_start().starts_with('{') {
            let result: SearchResult = match serde_json::from_str(body) {
                Ok(result) => result,
                Err(error) => {
                    log::debug!("{NAME}: unexpected _search JSON: {error:#}");
                    return Vec::new();
                }
            };
            let mut mangas = Vec::new();
            for fragment in result.manga_cards {
                let document = Html::parse_fragment(&fragment);
                mangas.extend(self.parse_cards(&document));
            }
            mangas
        } else {
            let document = Html::parse_document(body);
            self.parse_cards(&document)
        }
    }

    fn fetch_listing_page(
        &self,
        page: i64,
        query: Option<&str>,
        last_id: Option<&str>,
    ) -> Result<Vec<MangaInfo>> {
        let url = match (page, last_id) {
            (1, _) => match query {
                Some(query) => format!("{URL}/?q={}", urlencoding::encode(query)),
                None => URL.to_string(),
            },
            (_, Some(last_id)) => {
                let mut url = format!("{URL}/_search?p={}", urlencoding::encode(last_id));
                if let Some(query) = query {
                    url.push_str(&format!("&q={}", urlencoding::encode(query)));
                }
                url
            }
            (_, None) => URL.to_string(),
        };
        let body = self.fetch_text(&url)?;
        Ok(self.parse_cards_from_body(&body))
    }

    /// Walk the `_search?p=` chain to reach `page` (the API is offset-by-id,
    /// so page N needs the last id of every previous page).
    fn listing(&self, page: i64, query: Option<&str>) -> Result<Vec<MangaInfo>> {
        let target = page.clamp(1, MAX_LIST_PAGES);
        let mut last_id: Option<String> = None;
        for current in 1..=target {
            let mangas = self.fetch_listing_page(current, query, last_id.as_deref())?;
            if mangas.is_empty() {
                if current <= 1 {
                    bail!(
                        "{NAME}: no galleries found (search query {query:?}; markup may have changed)"
                    );
                }
                return Ok(Vec::new());
            }
            if current == target {
                return Ok(mangas);
            }
            last_id = mangas.last().map(|manga| manga.path.clone());
        }
        Ok(Vec::new())
    }

    fn details(&self, id: &str, document: &Html) -> Option<Details> {
        let mut details = parse_details(document)?;
        if details.cover_url.is_empty() {
            details.cover_url = format!("{URL}/_images/covers/{id}.jpg");
        }
        Some(details)
    }
}

impl Extension for RokuHentai {
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
        self.listing(page, None)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        // `supportsLatest = false` in Kotlin; the homepage is newest-first.
        self.listing(page, None)
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
            return self.listing(page, None);
        }
        self.listing(page, Some(query))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let id = gallery_id(&path)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: invalid gallery path {path:?}"))?;
        let document = self.fetch_page(&request_url(&id))?;
        let details = self
            .details(&id, &document)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: no details found for gallery {id}"))?;
        Ok(MangaInfo {
            source_id: ID,
            title: details.title,
            author: details.authors,
            genre: details.genres,
            status: Some("Completed".to_string()),
            description: details.description,
            path: id,
            cover_url: details.cover_url,
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let id = gallery_id(&path)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: invalid gallery path {path:?}"))?;
        let document = self.fetch_page(&request_url(&id))?;
        let details = parse_details(&document);
        Ok(vec![ChapterInfo {
            source_id: ID,
            title: details
                .as_ref()
                .map(|details| details.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "Gallery".to_string()),
            path: reader_path(&id),
            number: 0.0,
            scanlator: None,
            uploaded: details.map(|details| details.uploaded).unwrap_or(0),
        }])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = request_url(&path);
        let document = self.fetch_page(&url)?;

        let mut images: Vec<String> = Vec::new();
        for image in document.select(&selector("img.site-reader__image[data-src], img.site-reader__one-per-page-image[src]")) {
            let source = image
                .value()
                .attr("data-src")
                .or_else(|| image.value().attr("src"))
                .map(str::trim)
                .unwrap_or_default();
            if source.is_empty() {
                continue;
            }
            let image = if source.starts_with("https://") || source.starts_with("http://") {
                source.to_string()
            } else {
                format!("{URL}/{}", source.trim_start_matches('/'))
            };
            if !images.contains(&image) {
                images.push(image);
            }
        }
        if images.is_empty() {
            bail!("{NAME}: no page images found at {url} (markup may have changed)");
        }
        Ok(images)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        // The image endpoints answer 403 for requests without a site Referer.
        extension_utils::fetch_direct_image(&self.client, NAME, url, REFERER, NAME)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use scraper::Selector;

    #[test]
    fn parses_gallery_ids() {
        assert_eq!(gallery_id("https://rokuhentai.com/2aj8li/0").as_deref(), Some("2aj8li"));
        assert_eq!(gallery_id("/2aj8li/0").as_deref(), Some("2aj8li"));
        assert_eq!(gallery_id("2aj8li").as_deref(), Some("2aj8li"));
        assert_eq!(gallery_id(""), None);
    }

    #[test]
    fn extracts_background_images() {
        assert_eq!(
            background_image_url("background-image: url(\"https://rokuhentai.com/_images/covers/x.jpg\");")
                .as_deref(),
            Some("https://rokuhentai.com/_images/covers/x.jpg")
        );
        assert_eq!(
            background_image_url("background-image: url(https://rokuhentai.com/a.jpg)")
                .as_deref(),
            Some("https://rokuhentai.com/a.jpg")
        );
        assert_eq!(background_image_url("color: red;"), None);
    }

    #[test]
    fn parses_caption_dates() {
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 11, 17)
            .unwrap()
            .and_hms_opt(18, 16, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            parse_caption_date("11 images  Nov 17, 2026,  6:16 pm"),
            expected
        );
    }

    #[test]
    fn parses_json_fragment_cards() {
        let html = r#"<div class="mdc-layout-grid__cell"><div class="mdc-card">
            <a class="site-popunder-ad-slot" href="https://rokuhentai.com/chxx2p/0">
                <div class="mdc-card__primary-action">
                    <div class="mdc-card__media" style="background-image: url(&quot;https://rokuhentai.com/_images/cover-thumbnails/chxx2p.jpg&quot;);"></div>
                    <div class="site-manga-card__title--primary">Example title</div>
                </div>
            </a>
        </div></div>"#;
        let document = Html::parse_fragment(html);
        let cards = document
            .select(&Selector::parse("div.mdc-card").unwrap())
            .count();
        assert_eq!(cards, 1);
        assert_eq!(
            background_image_url(
                document
                    .select(&Selector::parse(".mdc-card__media").unwrap())
                    .next()
                    .and_then(|element| element.value().attr("style"))
                    .unwrap()
            )
            .as_deref(),
            Some("https://rokuhentai.com/_images/cover-thumbnails/chxx2p.jpg")
        );
    }
}
