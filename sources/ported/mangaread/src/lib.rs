//! MangaRead.org (`https://www.mangaread.org`) — Rust port of the Keiyoushi
//! Tachiyomi extension, which extends the shared `Madara` WordPress-theme
//! scraper.
//!
//! The site is a plain WordPress Madara install (no anti-bot handshake), so:
//! - archive listings (popular / latest / search) are fetched by POSTing
//!   `action=madara_load_more` to `wp-admin/admin-ajax.php`;
//! - the manga detail page `/manga/<slug>/` carries the whole chapter list
//!   inline (`li.wp-manga-chapter > a`), so no chapter ajax call is needed
//!   here; if a page ever renders the list lazily the `ajax/chapters/`
//!   endpoint is used as a fallback;
//! - chapter reading pages expose their images as `div.reading-content img`
//!   with a plain `src` whose value starts with tabs/newlines (trimmed);
//! - images live on the site host and are fetched with a site `Referer`; the
//!   bare host (no trailing slash) makes the site return a hotlink placeholder
//!   PNG, so every request sends the root with a trailing slash;
//! - a valid page image occasionally answers 404 from the site's CDN, so image
//!   downloads retry a few times before failing.
//!
//! All of the above was verified against the live site with curl.

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, MangaRead, NAME);

/// Keiyoushi source id (must stay unique across installed plugins).
const ID: i64 = 2681651342533848540;
const NAME: &str = "MangaRead.org";
const URL: &str = "https://www.mangaread.org";

/// Referer sent with every request. The site serves a hotlink placeholder image
/// when the Referer is the bare host without a path, so keep the trailing slash.
const REFERER: &str = "https://www.mangaread.org/";
const ICON_URL: &str = "https://www.mangaread.org/wp-content/uploads/2017/10/log1.png";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Madara lists manga under `/manga/` (the theme's `mangaSubString`).
const MANGA_SUBSTRING: &str = "manga";
const PAGE_SIZE: i64 = 25;

/// WordPress `admin-ajax.php`, used for archive listings.
fn ajax_url() -> String {
    format!("{URL}/wp-admin/admin-ajax.php")
}

pub struct MangaRead {
    client: RateLimitedAgent,
}

impl Default for MangaRead {
    fn default() -> Self {
        Self {
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

/// Headers matching the browser XHR the theme's own JS sends.
fn xhr_headers() -> [(&'static str, &'static str); 2] {
    [
        ("X-Requested-With", "XMLHttpRequest"),
        ("Referer", REFERER),
    ]
}

impl MangaRead {
    fn get_page(&self, url: &str) -> Result<Html> {
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", REFERER)])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn detail_url(slug: &str) -> String {
        format!("{URL}/{MANGA_SUBSTRING}/{slug}/")
    }

    /// One page of the Madara archive listing, filtered by `mode`.
    fn archive(&self, page: i64, mode: &str, query: &str) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let mut form: Vec<(&str, String)> = vec![
            ("action", "madara_load_more".to_string()),
            ("page", (page - 1).to_string()),
            ("template", "madara-core/content/content-archive".to_string()),
            ("vars[paged]", "1".to_string()),
            ("vars[template]", "archive".to_string()),
            ("vars[post_type]", "wp-manga".to_string()),
            ("vars[post_status]", "publish".to_string()),
            ("vars[posts_per_page]", PAGE_SIZE.to_string()),
            ("vars[manga_archives_item_layout]", "big_thumbnail".to_string()),
        ];

        match mode {
            "popular" => {
                form.push(("vars[orderby]", "meta_value_num".to_string()));
                form.push(("vars[meta_key]", "_wp_manga_views".to_string()));
                form.push(("vars[order]", "DESC".to_string()));
            }
            "latest" => {
                form.push(("vars[orderby]", "meta_value_num".to_string()));
                form.push(("vars[meta_key]", "_latest_update".to_string()));
                form.push(("vars[order]", "DESC".to_string()));
            }
            _ => {
                if !query.is_empty() {
                    form.push(("vars[s]", urlencoding::encode(query).into_owned()));
                }
            }
        }

        let borrowed: Vec<(&str, &str)> = form
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (body, _cookies) = self
            .client
            .post_form_with_headers(&ajax_url(), &borrowed, &xhr_headers())
            .context("requesting MangaRead.org archive listing")?;

        let fragment = Html::parse_fragment(&body);
        Ok(parse::archive_items(&fragment))
    }

    /// Detail page, or the chapter ajax fragment when the list is not inline.
    fn chapter_document(&self, slug: &str) -> Result<Html> {
        let url = Self::detail_url(slug);
        let document = self.get_page(&url)?;
        if parse::has_chapters(&document) {
            return Ok(document);
        }

        // Some Madara installs render `#manga-chapters-holder` lazily and the
        // theme's JS fills it from `ajax/chapters/`. This site normally embeds
        // the list, so this is only a safety net.
        let ajax = format!("{URL}/{MANGA_SUBSTRING}/{slug}/ajax/chapters/");
        log::debug!("{NAME}: chapter list missing inline, POSTing {ajax}");
        let (body, _cookies) = self
            .client
            .post_form_with_headers(&ajax, &[], &xhr_headers())
            .with_context(|| format!("requesting {ajax}"))?;
        Ok(Html::parse_document(&body))
    }

    fn chapter_url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{URL}/{}", path.trim_start_matches('/'))
        }
    }
}

impl Extension for MangaRead {
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

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.archive(page, "popular", "")
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.archive(page, "latest", "")
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        self.archive(page, "search", query.trim())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let slug = parse::slug(&path);
        let url = Self::detail_url(&slug);
        let document = self.get_page(&url)?;
        Ok(parse::manga_detail(&document, &slug))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let slug = parse::slug(&path);
        let document = self.chapter_document(&slug)?;
        Ok(parse::chapters(&document))
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = self.chapter_url(&path);
        let document = self.get_page(&url)?;
        let pages = parse::chapter_pages(&document)?;
        if pages.is_empty() {
            return Err(anyhow!("{NAME}: no pages found at {url}"));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        // The site's CDN occasionally answers 404 for a page image that exists
        // (observed while probing the live site), so retry a couple of times.
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match self
                .client
                .fetch_image_with_headers(&url, Some(REFERER), &[])
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!("{NAME}: image attempt {attempt} failed for {url}: {error:#}");
                    last_error = Some(error);
                    if attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(750));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("{NAME}: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}
