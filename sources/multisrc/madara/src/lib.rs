//! **Madara multisrc** — one Rust plugin that covers every WordPress-Madara
//! source from Keiyoushi (`lib-multisrc/madara`, ~240 site/source entries).
//!
//! A Tanoshi plugin registers a single source, so this binary is published once
//! per site in the extension catalog: every catalog entry points at the same
//! `.so` and the installed file name selects the site configuration embedded in
//! [`sites.json`](../sites.json). The server starts an extension worker with
//! `--plugin <path>`; the plugin therefore reads its own file name
//! (`lib<key>.so` / `<key>.so`) and looks the key up in the embedded table.
//!
//! Behaviour per site (verified against live Madara installs):
//! - archive listings come either from `wp-admin/admin-ajax.php`
//!   (`action=madara_load_more`) or, for `MadaraNoAjax` sites, from the
//!   `/{mangaSubString}/` HTML archive;
//! - chapter lists are inline on the detail page, or fetched from
//!   `<detail>/ajax/chapters/` (MangaAjax), `action=manga_get_chapters`
//!   (AdminAjax) or the paginated ajax endpoint (MangaAjaxPaginated);
//! - reading pages are `div.reading-content img` and are downloaded with a
//!   site `Referer` (retried a few times for flaky CDNs).

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, Madara, NAME);

const NAME: &str = "Madara";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const PAGE_SIZE: i64 = 25;
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `madara-toonily`.
    pub key: String,
    /// Display name shown by the server, e.g. `Toonily`.
    pub name: String,
    /// Site root, no trailing slash, e.g. `https://toonily.com`.
    pub url: String,
    /// Keiyoushi numeric source id (unique across plugins).
    pub id: i64,
    /// Keiyoushi language code (`en`, `es`, `all`, …).
    #[serde(default = "default_string_en")]
    pub lang: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub icon: Option<String>,
    /// Madara `mangaSubString` (`manga` by default).
    #[serde(default = "default_sub")]
    pub sub: String,
    /// `ajax` (default) or `html` (`MadaraNoAjax` sites).
    #[serde(default = "default_listing")]
    pub listing: String,
    /// `inline` (default), `ajax`, `admin` or `ajax_paginated`.
    #[serde(default = "default_chapters")]
    pub chapters: String,
}

fn default_string_en() -> String {
    "en".to_string()
}
fn default_sub() -> String {
    "manga".to_string()
}
fn default_listing() -> String {
    "ajax".to_string()
}
fn default_chapters() -> String {
    "inline".to_string()
}

impl Site {
    fn referer(&self) -> String {
        format!("{}/", self.url.trim_end_matches('/'))
    }

    fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn detail_url(&self, slug: &str) -> String {
        format!("{}/{}/{}/", self.base(), self.sub, slug)
    }

    fn icon(&self) -> String {
        self.icon
            .clone()
            .unwrap_or_else(|| format!("{}/favicon.ico", self.base()))
    }
}

static SITES: OnceLock<Vec<Site>> = OnceLock::new();

fn sites() -> &'static [Site] {
    SITES.get_or_init(|| {
        serde_json::from_str(include_str!("../sites.json"))
            .expect("embedded sites.json must be valid")
    })
}

/// Key of the installed plugin, from the env var tests use or the process
/// arguments (`--plugin <path>` in the server's worker, argv[1] in the harness).
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
/// read the kernel command line directly and fall back to `dladdr` on our own
/// code (which yields the loaded library path, including its symlink name).
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

pub struct Madara {
    site: Site,
    client: RateLimitedAgent,
}

impl Default for Madara {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        Self {
            site,
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
}

impl Madara {
    fn xhr_headers(&self) -> Vec<(&str, &str)> {
        vec![
            ("X-Requested-With", "XMLHttpRequest"),
            ("Referer", self.site.referer().leak()),
        ]
    }

    fn get_page(&self, url: &str) -> Result<Html> {
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.site.referer())])
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        Ok(Html::parse_document(&body))
    }

    fn post(&self, url: &str, form: &[(&str, &str)]) -> Result<Html> {
        let (body, _cookies) = self
            .client
            .post_form_with_headers(url, form, &self.xhr_headers())
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        Ok(Html::parse_document(&body))
    }

    /// One archive page: either the theme's admin-ajax endpoint or the HTML
    /// archive used by `MadaraNoAjax` sites.
    fn archive(&self, page: i64, mode: &str, query: &str) -> Result<Vec<MangaInfo>> {
        if self.site.listing == "html" {
            return self.archive_html(page, mode, query);
        }
        let items = self.archive_ajax(page, mode, query)?;
        if items.is_empty() {
            log::debug!("{NAME}: empty ajax archive for {}, retrying HTML", self.site.key);
            return self.archive_html(page, mode, query);
        }
        Ok(items)
    }

    fn archive_ajax(&self, page: i64, mode: &str, query: &str) -> Result<Vec<MangaInfo>> {
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
        let url = format!("{}/wp-admin/admin-ajax.php", self.site.base());
        let document = self.post(&url, &borrowed)?;
        Ok(parse::archive_items(&self.site, &document))
    }

    fn archive_html(&self, page: i64, mode: &str, query: &str) -> Result<Vec<MangaInfo>> {
        let base = self.site.base();
        let sub = &self.site.sub;
        let url = if mode == "search" && !query.is_empty() && page == 1 {
            format!("{base}/?s={}&post_type=wp-manga", urlencoding::encode(query))
        } else {
            let order = match mode {
                "popular" => "views",
                "latest" => "latest",
                _ => "",
            };
            let page_part = if page > 1 {
                format!("page/{page}/")
            } else {
                String::new()
            };
            if order.is_empty() {
                format!("{base}/{sub}/{page_part}")
            } else {
                format!("{base}/{sub}/{page_part}?m_orderby={order}")
            }
        };
        let document = self.get_page(&url)?;
        let items = parse::archive_items(&self.site, &document);
        if items.is_empty() && page == 1 {
            log::debug!("{NAME}: empty HTML archive for {}, retrying ajax", self.site.key);
            return self.archive_ajax(page, mode, query);
        }
        Ok(items)
    }

    fn fetch_chapters(&self, slug: &str) -> Result<Vec<ChapterInfo>> {
        let url = self.site.detail_url(slug);
        let document = self.get_page(&url)?;

        if parse::has_chapters(&document) {
            return Ok(parse::chapters(&self.site, &document));
        }

        let mut chapters: Vec<ChapterInfo> = Vec::new();
        match self.site.chapters.as_str() {
            "ajax" => {
                let ajax = format!("{}/ajax/chapters/", url.trim_end_matches('/'));
                let fragment = self.post(&ajax, &[])?;
                chapters = parse::chapters(&self.site, &fragment);
            }
            "admin" | "ajax_paginated" if self.site.chapters == "admin" => {
                let html = document.html();
                let id = parse::post_id(&html)
                    .ok_or_else(|| anyhow!("{}: auth ajax chapter id not found", self.site.key))?;
                let ajax = format!("{}/wp-admin/admin-ajax.php", self.site.base());
                let form = [
                    ("action", "manga_get_chapters"),
                    ("manga", &id.to_string()),
                ];
                let fragment = self.post(&ajax, &form)?;
                chapters = parse::chapters(&self.site, &fragment);
            }
            "ajax_paginated" => {
                for page in 1..=50 {
                    let ajax = format!("{}/ajax/chapters/?t={page}", url.trim_end_matches('/'));
                    let fragment = self.post(&ajax, &[])?;
                    let page_chapters = parse::chapters(&self.site, &fragment);
                    if page_chapters.is_empty()
                        || page_chapters.first().map(|c| &c.path) == chapters.last().map(|c| &c.path)
                    {
                        break;
                    }
                    chapters.extend(page_chapters);
                }
            }
            _ => {}
        }

        if chapters.is_empty() && self.site.chapters != "ajax" {
            let ajax = format!("{}/ajax/chapters/", url.trim_end_matches('/'));
            if let Ok(fragment) = self.post(&ajax, &[]) {
                chapters = parse::chapters(&self.site, &fragment);
            }
        }
        if chapters.is_empty() && self.site.chapters != "admin" {
            if let Some(id) = parse::post_id(&document.html()) {
                let ajax = format!("{}/wp-admin/admin-ajax.php", self.site.base());
                let id_string = id.to_string();
                let form = [("action", "manga_get_chapters"), ("manga", id_string.as_str())];
                if let Ok(fragment) = self.post(&ajax, &form) {
                    chapters = parse::chapters(&self.site, &fragment);
                }
            }
        }
        if chapters.is_empty() {
            log::warn!("{NAME}: no chapters found for {} ({})", self.site.key, url);
        }

        Ok(chapters)
    }
}

impl Extension for Madara {
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
        let url = self.site.detail_url(&slug);
        let document = self.get_page(&url)?;
        Ok(parse::manga_detail(&self.site, &document, &slug))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let slug = parse::slug(&path);
        self.fetch_chapters(&slug)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = if path.starts_with("http") {
            path.clone()
        } else {
            format!("{}/{}", self.site.base(), path.trim_start_matches('/'))
        };
        let document = self.get_page(&url)?;
        parse::chapter_pages(&self.site, &document)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let referer = self.site.referer();
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            let fetch = if attempt == 3 {
                self.client.fetch_image_with_headers(&url, None, &[])
            } else {
                self.client.fetch_image_with_headers(&url, Some(&referer), &[])
            };
            match fetch {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!(
                        "{NAME}: image attempt {attempt} failed for {url}: {error:#}"
                    );
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
