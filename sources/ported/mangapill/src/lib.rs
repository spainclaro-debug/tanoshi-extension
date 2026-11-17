//! MangaPill (`https://mangapill.com`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.en.mangapill`.
//!
//! The site is plain server-rendered HTML, so every operation is scraped:
//!
//! | operation | endpoint / selector |
//! |---|---|
//! | popular | `GET /` — the `Trending Mangas` grid |
//! | latest | `GET /chapters` — the `Recently Released Chapters` grid |
//! | search | `GET /search?page=&q=&status=&type=&genre=` — result grid |
//! | details | `GET /manga/{id}/{slug}` |
//! | chapters | `GET /manga/{id}/{slug}` — `#chapters > div > a` |
//! | pages | `GET /chapters/{id}-{chapter}/{slug}` — `picture img[data-src]` |
//! | images | `cdn.readdetectiveconan.com` — 403s without a site `Referer` |
//!
//! `MangaInfo.path` keeps the site-relative `manga/{id}/{slug}` form because the
//! numeric id is part of the detail URL; `ChapterInfo.path` keeps
//! `chapters/{id}-{chapter}/{slug}`. Both accessors accept either the stored
//! path, a root-relative path or a full URL.
//!
//! The homepage has no pagination, so the popular/latest listings return an
//! empty page 2 rather than repeating page 1; only `/search` is paginated.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod filters;
mod parse;

extension_utils::export_extension!(register, MangaPill, NAME);

const ID: i64 = 8448310129093543312;
const NAME: &str = "MangaPill";
const URL: &str = "https://mangapill.com";
const ICON_URL: &str = "https://mangapill.com/static/favicon/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct MangaPill {
    client: RateLimitedAgent,
}

impl Default for MangaPill {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

impl MangaPill {
    fn get_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn get_manga_page(&self, path: &str) -> Result<Html> {
        let url = parse::manga_url(path);
        self.get_page(&url)
    }
}

impl Extension for MangaPill {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn filter_list(&self) -> Vec<Input> {
        filters::filter_list()
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        if page > 1 {
            return Ok(Vec::new());
        }
        let document = self.get_page(&format!("{URL}/"))?;
        let mangas = parse::popular(&document);
        if mangas.is_empty() {
            bail!("{NAME}: no manga found in the homepage 'Trending Mangas' section");
        }
        Ok(mangas)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        if page > 1 {
            return Ok(Vec::new());
        }
        let document = self.get_page(&format!("{URL}/chapters"))?;
        let mangas = parse::listing(&document);
        if mangas.is_empty() {
            bail!("{NAME}: no manga found on the /chapters page");
        }
        Ok(mangas)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let query = query.unwrap_or_default();
        let url = parse::search_url(page, query.trim(), filters.as_deref());
        let document = self.get_page(&url)?;
        Ok(parse::listing(&document))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let document = self.get_manga_page(&path)?;
        Ok(parse::manga_detail(&document, &path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let document = self.get_manga_page(&path)?;
        let chapters = parse::chapters(&document);
        if chapters.is_empty() {
            bail!(
                "{NAME}: no chapters found for {path} (the chapter list markup may have changed)"
            );
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = parse::chapter_url(&path);
        let document = self.get_page(&url)?;
        let pages = parse::chapter_pages(&document);
        if pages.is_empty() {
            bail!("{NAME}: no page images found at {url}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        // The CDN rejects requests without a mangapill.com Referer (403).
        extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME)
    }
}
