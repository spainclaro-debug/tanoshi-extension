//! **Masonry multisrc** — one Rust plugin covering the Keiyoushi `masonry`
//! multisrc theme (Elite Babes and the six Hunter sites).
//!
//! A Tanoshi plugin registers a single source, so this binary is published once
//! per site in the extension catalog: every catalog entry points at the same
//! `.so` and the installed file name selects the site configuration embedded in
//! [`sites.json`](../sites.json). The server starts an extension worker with
//! `--plugin <path>`; `std::env::args()` is empty inside a `dlopen`ed library on
//! Android/Termux, so the plugin reads `/proc/self/cmdline`, falls back to
//! `dladdr` on its own code and finally to `TANOSHI_MULTISRC_SITE` (tests).
//!
//! Theme behaviour (mirrored from `lib-multisrc/masonry/Masonry.kt`, verified
//! against the live sites):
//! - listings use the theme's infinite-grid JSON-free feed
//!   `GET /gridapi/?content=updates&sort=popular|newest&mpage=<page>`; it returns
//!   the same `<li><figure>` cards as the full pages (`/`, `/archive/`,
//!   `/sort/newest/`) but paginates reliably. Note the theme's Kotlin
//!   `/updates/sort/newest/mpage/N/` URLs currently redirect to page 1; the
//!   site's own `/sort/newest/` page advertises the working pagination through
//!   that grid API;
//! - text search is `GET /search/post/<query>/mpage/<page>/`;
//! - galleries are single-chapter works (chapter "Gallery" == gallery page);
//! - the detail page lists the full-size photos as
//!   `.list-gallery.static a[href^=https://cdn.]`, which are fetched with the
//!   site `Referer`;
//! - video posts (a play badge on the listing card, or a `<video>` player on the
//!   page) have no photo set, so they are skipped instead of opening zero pages.
//!
//! All seven sites were verified against the live sites.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, Masonry, NAME);

const NAME: &str = "Masonry";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `masonry-elitebabes`.
    pub key: String,
    /// Display name shown by the server, e.g. `Elite Babes`.
    pub name: String,
    /// Site root, no trailing slash, e.g. `https://www.elitebabes.com`.
    pub url: String,
    /// Keiyoushi numeric source id (unique across plugins).
    pub id: i64,
    /// Keiyoushi language code (`all` for this theme).
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub icon: String,
}

impl Site {
    pub(crate) fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn referer(&self) -> String {
        format!("{}/", self.base())
    }

    fn icon(&self) -> String {
        if self.icon.is_empty() {
            format!("{}/favicon.ico", self.base())
        } else {
            self.icon.clone()
        }
    }

    fn absolute(&self, path: &str) -> String {
        let path = path.trim();
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}/{}", self.base(), path.trim_start_matches('/'))
        }
    }

    /// `GET /gridapi/?content=updates&sort=...&mpage=N` for the infinite grid.
    fn grid_url(&self, sort: &str, page: i64) -> String {
        let page = page.max(1);
        format!(
            "{}/gridapi/?content=updates&sort={sort}&mpage={page}",
            self.base()
        )
    }

    fn search_url(&self, page: i64, query: &str) -> String {
        let page = page.max(1);
        format!(
            "{}/search/post/{}/mpage/{page}/",
            self.base(),
            urlencoding::encode(query)
        )
    }
}

static SITES: OnceLock<Vec<Site>> = OnceLock::new();

fn sites() -> &'static [Site] {
    SITES.get_or_init(|| {
        serde_json::from_str(include_str!("../sites.json"))
            .expect("embedded sites.json must be valid")
    })
}

/// Key of the installed plugin from a file path (`lib<key>.so` / `<key>.so`).
fn key_from_path(path: &str) -> Option<String> {
    let file = std::path::Path::new(path)
        .file_name()?
        .to_string_lossy()
        .to_string();
    let stem = file
        .strip_suffix(".so")
        .or_else(|| file.strip_suffix(".SO"))
        .unwrap_or(&file);
    let stem = stem.strip_prefix("lib").unwrap_or(stem);
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

/// Arguments of the current process.
///
/// `std::env::args()` is empty inside a `dlopen`ed library on Android/Termux, so
/// read the kernel command line directly.
fn process_args() -> Vec<String> {
    if let Ok(bytes) = std::fs::read("/proc/self/cmdline") {
        let args: Vec<String> = bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        if !args.is_empty() {
            return args;
        }
    }
    Vec::new()
}

fn own_library_path() -> Option<String> {
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        let address = selected_key as *const () as *mut libc::c_void;
        if libc::dladdr(address, &mut info) != 0 && !info.dli_fname.is_null() {
            let name = std::ffi::CStr::from_ptr(info.dli_fname).to_string_lossy();
            return Some(name.into_owned());
        }
    }
    None
}

fn selected_key() -> Option<String> {
    if let Ok(key) = std::env::var("TANOSHI_MULTISRC_SITE") {
        if !key.trim().is_empty() {
            return Some(key.trim().to_string());
        }
    }
    let args = process_args();
    log::debug!("{NAME}: process arguments: {args:?}");
    if let Some(path) = args
        .iter()
        .position(|arg| arg == "--plugin")
        .and_then(|index| args.get(index + 1))
    {
        if let Some(key) = key_from_path(path) {
            return Some(key);
        }
    }
    if let Some(key) = args
        .iter()
        .find(|arg| arg.ends_with(".so") || arg.ends_with(".SO"))
        .and_then(|path| key_from_path(path))
    {
        return Some(key);
    }
    own_library_path().and_then(|path| key_from_path(&path))
}

fn selected_site() -> Site {
    let Some(key) = selected_key() else {
        let site = sites().first().cloned().expect("sites.json must not be empty");
        log::warn!("{NAME}: no plugin name detected, using {}", site.key);
        return site;
    };
    match sites().iter().find(|site| site.key == key) {
        Some(site) => site.clone(),
        None => {
            let site = sites().first().cloned().expect("sites.json must not be empty");
            log::warn!("{NAME}: unknown site key {key}, using {}", site.key);
            site
        }
    }
}

pub struct Masonry {
    site: Site,
    client: RateLimitedAgent,
}

impl Default for Masonry {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        Self {
            site,
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
}

impl Masonry {
    fn get_html(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let referer = self.site.referer();
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", referer.as_str())])
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        Ok(Html::parse_document(&body))
    }

    fn listing(&self, url: &str) -> Result<Vec<MangaInfo>> {
        let document = self.get_html(url)?;
        let items = parse::listing(&self.site, &document);
        if items.is_empty() {
            log::debug!("{NAME}: empty listing for {} at {url}", self.site.key);
        }
        Ok(items)
    }
}

impl Extension for Masonry {
    fn get_source_info(&self) -> SourceInfo {
        let icon: &'static str = Box::leak(self.site.icon().into_boxed_str());
        let languages = if self.site.lang == "all" {
            Lang::All
        } else {
            Lang::Single(self.site.lang.clone())
        };
        SourceInfo {
            id: self.site.id,
            name: self.site.name.clone(),
            url: self.site.url.clone(),
            version: VERSION,
            icon,
            languages,
            nsfw: self.site.nsfw,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.listing(&self.site.grid_url("popular", page))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.listing(&self.site.grid_url("newest", page))
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
            return self.get_popular_manga(page);
        }
        self.listing(&self.site.search_url(page, query))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let url = self.site.absolute(&path);
        let document = self.get_html(&url)?;
        Ok(parse::detail(&self.site, &document, &path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        // A gallery is a single-chapter work; the chapter path is the gallery
        // page itself, so no extra request is needed.
        let chapters = parse::chapters(&self.site, &path);
        if chapters.is_empty() {
            return Err(anyhow!("{}: no chapters found at {path}", self.site.key));
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = self.site.absolute(&path);
        let document = self.get_html(&url)?;
        let pages = parse::pages(&self.site, &document);
        if pages.is_empty() {
            return Err(anyhow!("{}: no pages found at {url}", self.site.key));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let referer = self.site.referer();
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            let fetch = if attempt < 3 {
                self.client
                    .fetch_image_with_headers(&url, Some(&referer), &[])
            } else {
                self.client.fetch_image_with_headers(&url, None, &[])
            };
            match fetch {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!("{NAME}: image attempt {attempt} failed for {url}: {error:#}");
                    last_error = Some(error);
                    if attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("{NAME}: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}
