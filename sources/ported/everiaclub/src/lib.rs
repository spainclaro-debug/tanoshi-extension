//! Everia.club (`https://everia.club`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.everiaclub`.
//!
//! WordPress site: listings come from the REST API, popular from a homepage
//! widget, and photo sets are scraped from post HTML.
//!
//! | operation | endpoint |
//! |---|---|
//! | popular | `GET /` — `.wli_popular_posts-class li` widget |
//! | latest | `GET /wp-json/wp/v2/posts?page=&per_page=20&_embed=wp:featuredmedia` |
//! | search | same endpoint with `search=` / `categories=` |
//! | details | `GET /{path}` — `.entry-title`, `.post-tags a` |
//! | chapters | one `Gallery` entry per photo set |
//! | pages | `GET /{path}` — `.entry-content img` plus `.page-links` sub-pages |
//! | images | `karubox.top` / `remu.airibox.top` — no special headers required |
//!
//! The original also exposes a dynamic tri-state tag group; Tanoshi's
//! `filter_list()` is synchronous, so this port only ships the (stable)
//! category filter and keeps free-text search.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod dto;
mod filters;
mod parse;

use dto::WPPostDto;

extension_utils::export_extension!(register, EveriaClub, NAME);

const ID: i64 = 7698513740234984368;
const NAME: &str = "Everia.club";
const URL: &str = "https://everia.club";
const ICON_URL: &str =
    "https://everia.club/wp-content/uploads/2026/05/cropped-everiaicon-192x192.webp";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const PER_PAGE: i64 = 20;
/// Guard against a runaway multi-page photo set.
const MAX_SUB_PAGES: usize = 20;

pub struct EveriaClub {
    client: RateLimitedAgent,
}

impl Default for EveriaClub {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

impl EveriaClub {
    fn get_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn get_json(&self, url: &str) -> Result<String> {
        log::debug!("{NAME}: GET {url}");
        self.client
            .fetch_text_with_headers(
                url,
                &[
                    ("Referer", URL),
                    ("X-Requested-With", "XMLHttpRequest"),
                    ("Accept", "application/json"),
                ],
            )
            .with_context(|| format!("requesting {url}"))
    }

    /// One page of the posts REST endpoint, optionally filtered.
    fn posts(&self, page: i64, query: &str, category: Option<&str>) -> Result<Vec<MangaInfo>> {
        let mut url = format!(
            "{URL}/wp-json/wp/v2/posts?page={}&per_page={PER_PAGE}&_embed=wp:featuredmedia",
            page.max(1)
        );
        if !query.is_empty() {
            url.push_str(&format!("&search={}", urlencoding::encode(query)));
        }
        if let Some(category) = category.filter(|category| !category.is_empty()) {
            url.push_str(&format!("&categories={}", urlencoding::encode(category)));
        }

        let body = self.get_json(&url)?;
        let posts: Vec<WPPostDto> =
            serde_json::from_str(&body).context("decoding Everia.club posts")?;
        Ok(posts.into_iter().map(WPPostDto::into_manga).collect())
    }
}

impl Extension for EveriaClub {
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
        if page > 1 {
            // The homepage widget is a fixed top-8 list with no pagination.
            return Ok(Vec::new());
        }
        Ok(parse::popular(&self.get_page(&format!("{URL}/"))?))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.posts(page, "", None)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let query = query.trim();
        let category = filters.as_deref().and_then(filters::category);
        self.posts(page, query, category.as_deref())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let url = parse::manga_url(&path);
        Ok(parse::manga_detail(&self.get_page(&url)?, &path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let url = parse::manga_url(&path);
        Ok(parse::chapters(&self.get_page(&url)?, &path))
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = parse::manga_url(&path);
        let document = self.get_page(&url)?;
        let mut pages = parse::gallery_images(&document);

        for sub_page in parse::page_links(&document).into_iter().take(MAX_SUB_PAGES) {
            let Ok(document) = self.get_page(&sub_page) else {
                log::warn!("{NAME}: failed to fetch gallery sub-page {sub_page}");
                continue;
            };
            for image in parse::gallery_images(&document) {
                if !pages.contains(&image) {
                    pages.push(image);
                }
            }
        }

        if pages.is_empty() {
            bail!("{NAME}: no gallery images found at {url}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}
