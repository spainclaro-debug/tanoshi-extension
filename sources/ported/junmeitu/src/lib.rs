//! Junmeitu (`https://meijuntu.com`) — Rust port of the Keiyoushi Tachiyomi
//! extension `eu.kanade.tachiyomi.extension.all.junmeitu`.
//!
//! | operation | endpoint |
//! |---|---|
//! | popular | `GET /beauty/hot-{page}.html` |
//! | latest | `GET /beauty/index-{page}.html` |
//! | search | `/search/{q}-{page}.html`, `/tags/…`, `/model/…`, `/xzjg/…`, `/{cat}/{sort}-{page}.html` |
//! | details | `GET /{path}` — `.title`, `.base-info`, `.picture-details` |
//! | chapters | one `Gallery` entry per photo set |
//! | pages | `GET /{path}` — `.pages` count and the `pc_cid`/`pc_id` script |
//! | images | `s.wujituku.com` / `i.wujituku.com` — no special headers required |
//!
//! The big galleries paginate one image per page (`…-2.html` … `…-126.html`),
//! and the image URL of every follow-up page is only obtainable from that
//! page's AJAX response (`/ajax_beauty/…?ajax=1&catid=&conid=`). Resolving all
//! of them inside `get_pages` would blow the server's 30 s metadata timeout for
//! a 126-image set, so [`Extension::get_pages`] returns the per-page URLs and
//! [`Extension::get_image_bytes`] resolves each one on demand — the same
//! `getImageUrl` split the original extension uses.

use anyhow::{Context, Result, bail};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod dto;
mod filters;
mod parse;

use dto::Dto;

extension_utils::export_extension!(register, Junmeitu, NAME);

const ID: i64 = 4721197766605490540;
const NAME: &str = "Junmeitu";
const URL: &str = "https://meijuntu.com";
const ICON_URL: &str = "https://meijuntu.com/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Guard against a runaway photo set.
const MAX_GALLERY_PAGES: i64 = 300;

pub struct Junmeitu {
    client: RateLimitedAgent,
}

impl Default for Junmeitu {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

impl Junmeitu {
    fn get_page(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", URL)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn get_text(&self, url: &str) -> Result<String> {
        log::debug!("{NAME}: GET {url}");
        self.client
            .fetch_text_with_headers(
                url,
                &[("Referer", URL), ("X-Requested-With", "XMLHttpRequest")],
            )
            .with_context(|| format!("requesting {url}"))
    }
}

impl Extension for Junmeitu {
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
        let url = format!("{URL}/beauty/hot-{}.html", page.max(1));
        Ok(parse::listing(&self.get_page(&url)?))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = format!("{URL}/beauty/index-{}.html", page.max(1));
        Ok(parse::listing(&self.get_page(&url)?))
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
        let filters = filters
            .as_deref()
            .map(filters::SearchFilters::parse)
            .unwrap_or_default();
        let url = filters.search_url(page, query);
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
        let pages = parse::pages(&document, &url);
        if pages.is_empty() {
            bail!("{NAME}: no pages found at {url}");
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        // Follow-up gallery pages are AJAX endpoints whose JSON wraps the image
        // in an HTML `<img>` snippet; resolve that first.
        if parse::is_ajax_page(&url) {
            let body = self.get_text(&url)?;
            let data: Dto = serde_json::from_str(&body)
                .with_context(|| format!("decoding gallery AJAX response from {url}"))?;
            let image = parse::first_image_src(&data.pic)
                .ok_or_else(|| anyhow::anyhow!("{NAME}: AJAX response has no image for {url}"))?;
            return extension_utils::fetch_direct_image(&self.client, NAME, image, URL, NAME);
        }

        // Plain image URLs (inline `.news-body` galleries) go straight through.
        if parse::looks_like_image(&url) {
            return extension_utils::fetch_direct_image(&self.client, NAME, url, URL, NAME);
        }

        // A gallery HTML page: pull its `.pictures img` source.
        let document = self.get_page(&url)?;
        let image = parse::first_picture(&document)
            .ok_or_else(|| anyhow::anyhow!("{NAME}: no picture found at {url}"))?;
        extension_utils::fetch_direct_image(&self.client, NAME, image, URL, NAME)
    }
}
