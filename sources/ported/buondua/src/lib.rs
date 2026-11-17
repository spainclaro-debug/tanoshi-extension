//! Buon Dua (`https://buondua.com`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.buondua`.
//!
//! Server-rendered photo-set site (no JSON API):
//!
//! | operation | endpoint |
//! |---|---|
//! | popular | `GET /hot?start=20*(page-1)` |
//! | latest | `GET /?start=20*(page-1)` |
//! | search | `GET /?search=...&start=...` (tag filter: `GET /tag/{id}?start=...`) |
//! | details | `GET /{slug}` — `.article-header`, `.article-info`, `.article-tags` |
//! | chapters | one `Gallery` entry per photo set |
//! | pages | `GET /{slug}?page=N` — `.article-fulltext img` across all pages |
//! | images | `i2.buondua.com` — no special headers required |
//!
//! A photo set is paginated with 20 images per page; the original extension
//! either splits those pages into separate chapters or merges them. This port
//! always merges, so a set is one chapter with every image as a page, mirroring
//! the original's `splitPages = false` mode.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod filters;
mod parse;

extension_utils::export_extension!(register, BuonDua, NAME);

const ID: i64 = 1435103006121310663;
const NAME: &str = "Buon Dua";
const URL: &str = "https://buondua.com";
const ICON_URL: &str = "https://buondua.com/templates/duachua/watermelon-icon.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Listings show 20 cards per page (`start` is a multiple of 20).
const PAGE_SIZE: i64 = 20;
/// Guard against a runaway gallery merge (the site paginates 20 images/page).
const MAX_GALLERY_PAGES: i64 = 60;

pub struct BuonDua {
    client: RateLimitedAgent,
}

impl Default for BuonDua {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                // The original throttles the site to 10 requests/second.
                Some(10.0),
            ),
        }
    }
}

impl BuonDua {
    fn get_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn start_offset(page: i64) -> i64 {
        PAGE_SIZE * (page.max(1) - 1)
    }
}

impl Extension for BuonDua {
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

    fn filter_list(&self) -> Vec<Input> {
        filters::filter_list()
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = format!("{URL}/hot?start={}", Self::start_offset(page));
        Ok(parse::listing(&self.get_page(&url)?))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = format!("{URL}/?start={}", Self::start_offset(page));
        Ok(parse::listing(&self.get_page(&url)?))
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let start = Self::start_offset(page);
        let query = query.unwrap_or_default();
        let query = query.trim();
        let tag = filters
            .as_deref()
            .and_then(filters::tag_id)
            .filter(|tag| !tag.is_empty());

        // The original prefers the keyword, then the tag filter, then popular.
        let url = if !query.is_empty() {
            format!("{URL}/?search={}&start={start}", urlencoding::encode(query))
        } else if let Some(tag) = tag {
            format!("{URL}/tag/{}?start={start}", urlencoding::encode(&tag))
        } else {
            format!("{URL}/hot?start={start}")
        };

        Ok(parse::listing(&self.get_page(&url)?))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let url = parse::manga_url(&path);
        Ok(parse::manga_detail(&self.get_page(&url)?, &path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let url = parse::manga_url(&path);
        let document = self.get_page(&url)?;
        let chapters = parse::chapters(&document, &path);
        if chapters.is_empty() {
            bail!("{NAME}: no gallery found at {url}");
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let base = parse::manga_url(&path);
        let first = self.get_page(&base)?;
        let max_page = parse::last_page(&first).clamp(1, MAX_GALLERY_PAGES);

        let mut pages = parse::gallery_images(&first);
        for page in 2..=max_page {
            let url = format!("{base}?page={page}");
            let document = self
                .get_page(&url)
                .with_context(|| format!("requesting gallery page {page} of {base}"))?;
            for image in parse::gallery_images(&document) {
                if !pages.contains(&image) {
                    pages.push(image);
                }
            }
        }

        if pages.is_empty() {
            bail!("{NAME}: no images found at {base}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}
