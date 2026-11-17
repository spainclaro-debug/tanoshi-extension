//! FoamGirl (`https://foamgirl.net`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.foamgirl`.
//!
//! | operation | endpoint |
//! |---|---|
//! | popular | `GET /page/{page}` — `.update_area .i_list` cards |
//! | latest | same feed (the site only exposes this update-ordered listing) |
//! | search | `GET /page/{page}?post_type=post&s=...` |
//! | details | `GET /{id}.html` — `h1`, `.image-info-time`, `.image-info-cat` |
//! | chapters | one `GALLERY` entry per photo set |
//! | pages | `.imageclick-imgbox` links across the `.page-numbers` pages |
//! | images | `cdn.foamgirl.net` — no special headers required |
//!
//! The original extension marks "latest" unsupported because the site has no
//! separate latest feed; `/page/N` is already ordered by update time, so this
//! port exposes it as both popular and latest instead of failing.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, FoamGirl, NAME);

const ID: i64 = 5874068195320206396;
const NAME: &str = "FoamGirl";
const URL: &str = "https://foamgirl.net";
const ICON_URL: &str = "https://foamgirl.net/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Guard against a runaway gallery loop.
const MAX_GALLERY_PAGES: usize = 64;

pub struct FoamGirl {
    client: RateLimitedAgent,
}

impl Default for FoamGirl {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                // The original throttles the site to 3 requests/second.
                Some(3.0),
            ),
        }
    }
}

impl FoamGirl {
    fn get_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn listing(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = format!("{URL}/page/{}", page.max(1));
        Ok(parse::listing(&self.get_page(&url)?))
    }
}

impl Extension for FoamGirl {
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
        self.listing(page)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        // `/page/N` is the site's update-ordered feed.
        self.listing(page)
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
            return self.listing(page);
        }

        let url = format!(
            "{URL}/page/{page}?post_type=post&s={}",
            urlencoding::encode(query)
        );
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
        let mut url = parse::manga_url(&path);
        let mut pages: Vec<String> = Vec::new();
        let mut visited: Vec<String> = Vec::new();

        loop {
            visited.push(url.clone());
            let document = self.get_page(&url)?;
            for image in parse::gallery_images(&document) {
                if !pages.contains(&image) {
                    pages.push(image);
                }
            }

            match parse::next_page(&document) {
                Some(next) if !visited.contains(&next) && visited.len() < MAX_GALLERY_PAGES => {
                    url = next;
                }
                _ => break,
            }
        }

        if pages.is_empty() {
            bail!("{NAME}: no images found at {url}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}
