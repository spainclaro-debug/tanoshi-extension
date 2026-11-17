//! CosplayTele (`https://cosplaytele.com`) — Rust port of the Keiyoushi
//! Tachiyomi extension `eu.kanade.tachiyomi.extension.all.cosplaytele`.
//!
//! WordPress site: popular comes from the popular-posts plugin REST endpoint,
//! everything else is scraped from HTML.
//!
//! | operation | endpoint |
//! |---|---|
//! | popular | `GET /wp-json/wordpress-popular-posts/v1/popular-posts` |
//! | latest | `GET /page/{page}/` — `main div.box` cards |
//! | search | `GET /page/{page}/?s=...` or a category archive |
//! | details | `GET /{slug}` — `.entry-title` |
//! | chapters | one `Gallery` entry per photo set |
//! | pages | `GET /{slug}` — `.gallery-item img` |
//! | images | `wp-content/uploads` — no special headers required |
//!
//! The original used `offset = page * 20` for popular, which skips the first
//! 20 posts on page 1; this port uses the conventional `(page - 1) * 20`.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod dto;
mod filters;
mod parse;

use dto::PopularPostDto;

extension_utils::export_extension!(register, CosplayTele, NAME);

const ID: i64 = 7395748658970167454;
const NAME: &str = "CosplayTele";
const URL: &str = "https://cosplaytele.com";
const ICON_URL: &str =
    "https://cosplaytele.com/wp-content/uploads/2024/01/cropped-icon-cosplaytele-1-192x192.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const POPULAR_LIMIT: i64 = 20;

pub struct CosplayTele {
    client: RateLimitedAgent,
}

impl Default for CosplayTele {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

impl CosplayTele {
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

    fn latest_page(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = format!("{URL}/page/{}/", page.max(1));
        Ok(parse::listing(&self.get_page(&url)?))
    }
}

impl Extension for CosplayTele {
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
        let page = page.max(1);
        let url = format!(
            "{URL}/wp-json/wordpress-popular-posts/v1/popular-posts?offset={}&limit={POPULAR_LIMIT}&range=last7days&embed=true&_embed=wp:featuredmedia&_fields=title,link,_embedded,_links.wp:featuredmedia",
            (page - 1) * POPULAR_LIMIT
        );
        let body = self.get_json(&url)?;
        let posts: Vec<PopularPostDto> =
            serde_json::from_str(&body).context("decoding CosplayTele popular posts")?;
        Ok(posts.into_iter().map(PopularPostDto::into_manga).collect())
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.latest_page(page)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let query = query.unwrap_or_default();
        let query = query.trim();
        let category = filters.as_deref().and_then(filters::category);
        let has_category = category.as_deref().is_some_and(|slug| !slug.is_empty());

        let url = if has_category {
            let mut url = format!(
                "{URL}/{}/page/{page}",
                category.unwrap_or_default().trim_matches('/')
            );
            if !query.is_empty() {
                url.push_str(&format!("?s={}", urlencoding::encode(query)));
            }
            url
        } else if !query.is_empty() {
            format!("{URL}/page/{page}/?s={}", urlencoding::encode(query))
        } else {
            return self.latest_page(page);
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
        let url = parse::manga_url(&path);
        let document = self.get_page(&url)?;
        let pages = parse::gallery_images(&document);
        if pages.is_empty() {
            bail!("{NAME}: no gallery images found at {url}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}
