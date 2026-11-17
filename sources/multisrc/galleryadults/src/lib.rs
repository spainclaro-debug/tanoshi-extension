//! **GalleryAdults multisrc** — one Rust plugin covering every Keiyoushi
//! `galleryadults`-theme source (`lib-multisrc/galleryadults`, 57 site/language
//! entries across 11 sites).
//!
//! A Tanoshi plugin registers a single source, so this binary is published once
//! per site in the extension catalog: every catalog entry points at the same
//! `.so` and the installed file name selects the site configuration embedded in
//! [`sites.json`](../sites.json). On Android/Termux the server starts the worker
//! with `--plugin <path>`; `std::env::args()` is empty inside a `dlopen`ed
//! library, so the plugin reads `/proc/self/cmdline`, falls back to `dladdr` on
//! its own code and finally to `TANOSHI_MULTISRC_SITE` (tests).
//!
//! Common theme behaviour (mirrored from `GalleryAdults.kt`):
//! - galleries are single-chapter works; the chapter path is the gallery page;
//! - listings come from `/language/<lang>/[popular]`, `/search/`, `/new/` or
//!   the site-specific search endpoints (see [`sites`]);
//! - page lists are resolved from the gallery page's embedded JSON metadata
//!   (`$.parseJSON` / reader JSON), the thumbnail grid + `inc/thumbs_loader.php`
//!   endpoint, or the cover directory (per site, see [`sites::pages`]);
//! - images are fetched with the site `Referer`.
//!
//! Every site/language entry was verified against the live site.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;
mod sites;

extension_utils::export_extension!(register, GalleryAdults, NAME);

const NAME: &str = "GalleryAdults";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `galleryadults-asmhentai-en`.
    pub key: String,
    /// Display name shown by the server, e.g. `AsmHentai`.
    pub name: String,
    /// Site root, no trailing slash, e.g. `https://asmhentai.com`.
    pub url: String,
    /// Keiyoushi numeric source id (unique across plugins).
    pub id: i64,
    /// Keiyoushi language code (`en`, `ja`, `all`, …).
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub icon: String,
    /// Theme site slug (`asmhentai`, `hentaienvy`, …).
    pub slug: String,
}

impl Site {
    fn base(&self) -> String {
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
}

pub(crate) fn mangalang(site: &Site) -> &'static str {
    sites::mangalang(site)
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
    log::info!("{NAME}: process arguments: {args:?}");
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

/// Parse the date strings used across the theme's detail pages.
pub(crate) fn parse_date_to_epoch(raw: &str) -> Option<i64> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
        return Some(date.timestamp());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
        "%d/%m/%Y %I:%M %p",
    ] {
        if let Ok(date) = NaiveDateTime::parse_from_str(raw, format) {
            return Some(Utc.from_utc_datetime(&date).timestamp());
        }
    }
    for format in ["%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|date| Utc.from_utc_datetime(&date).timestamp());
        }
    }
    let lower = raw.to_ascii_lowercase();
    if lower.contains("just now") || lower.starts_with("today") {
        return Some(Utc::now().timestamp());
    }
    if lower.starts_with("yesterday") {
        return Some(Utc::now().timestamp() - 86_400);
    }
    // "3 years ago", "21 hours ago", "2 days ago (Updated …)".
    let number: Option<i64> = lower
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok());
    let number = number.or_else(|| {
        if lower.contains("one") || lower.contains("a ") {
            Some(1)
        } else {
            None
        }
    })?;
    let seconds = if lower.contains("second") || lower.contains("segundo") {
        1
    } else if lower.contains("minute") || lower.contains("minuto") || lower.contains("min ") {
        60
    } else if lower.contains("hour") || lower.contains("hora") {
        3_600
    } else if lower.contains("day") || lower.contains("día") || lower.contains("dia ") {
        86_400
    } else if lower.contains("week") || lower.contains("semana") {
        604_800
    } else if lower.contains("month") || lower.contains("mes ") {
        2_592_000
    } else if lower.contains("year") || lower.contains("año") {
        31_536_000
    } else {
        return None;
    };
    Some(Utc::now().timestamp() - number * seconds)
}

pub struct GalleryAdults {
    site: Site,
    client: RateLimitedAgent,
}

impl Default for GalleryAdults {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        // imhentai.xxx sits behind Cloudflare bot management, which answers
        // HTTP 403 to a browser User-Agent sent over a non-browser TLS
        // fingerprint; identifying honestly gets through. Every other site of
        // the theme is happy with (and sometimes prefers) the browser UA.
        let user_agent = if site.slug == "imhentai" {
            None
        } else {
            Some(USER_AGENT)
        };
        Self {
            site,
            client: build_rate_limited_ureq_agent(user_agent, None),
        }
    }
}

impl GalleryAdults {
    fn get_html(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let referer = self.site.referer();
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", referer.as_str())])
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        Ok(Html::parse_document(&body))
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<String> {
        log::debug!("{NAME}: POST {url}");
        let referer = self.site.referer();
        let headers = [
            ("X-Requested-With", "XMLHttpRequest"),
            ("Referer", referer.as_str()),
        ];
        let (body, _cookies) = self
            .client
            .post_form_with_headers(url, form, &headers)
            .with_context(|| format!("{}: posting {url}", self.site.key))?;
        Ok(body)
    }

    fn chapter_document(&self, path: &str) -> Result<Html> {
        let url = sites::absolute(&self.site, path);
        self.get_html(&url)
    }
}

impl Extension for GalleryAdults {
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
        let url = sites::popular_url(&self.site, page);
        let html = self.get_html(&url)?;
        Ok(parse::listing(&self.site, &html))
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let url = sites::latest_url(&self.site, page);
        let html = self.get_html(&url)?;
        Ok(parse::listing(&self.site, &html))
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let query = query.trim();
        let url = sites::search_url(&self.site, page, query);
        let html = self.get_html(&url)?;
        let results = parse::search_listing(&self.site, &html);
        if !results.is_empty() {
            return Ok(results);
        }
        // A language-restricted search on an intermediate endpoint can return
        // nothing when the site has no match in that language; retry without
        // the language filter so the source still returns real results.
        log::debug!("{NAME}: no match for {query:?} in {}, retrying unfiltered", self.site.lang);
        let fallback = sites::search_url_multi(&self.site, page, query);
        let html = self.get_html(&fallback)?;
        Ok(parse::search_listing(&self.site, &html))
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let html = self.chapter_document(&path)?;
        Ok(parse::detail(&self.site, &html, &path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let html = self.chapter_document(&path)?;
        let chapters = parse::chapter(&self.site, &path, &html);
        if chapters.is_empty() {
            return Err(anyhow!("{}: no chapters found at {path}", self.site.key));
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let get = |url: &str| self.get_html(url);
        let post = |url: &str, form: &[(&str, &str)]| self.post_form(url, form);
        let pages = sites::pages(&self.site, &path, &get, &post)?;
        if pages.is_empty() {
            return Err(anyhow!("{}: no pages found at {path}", self.site.key));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let (primary, fallbacks) = split_image_fallbacks(&url);
        let referer = self.site.referer();
        let mut candidates: Vec<String> = vec![primary];
        candidates.extend(fallbacks);
        let mut last_error: Option<anyhow::Error> = None;
        for candidate in &candidates {
            for attempt in 1..=2u32 {
                let result = if attempt == 1 {
                    self.client
                        .fetch_image_with_headers(candidate, Some(&referer), &[])
                } else {
                    self.client.fetch_image_with_headers(candidate, None, &[])
                };
                match result {
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => {
                        log::debug!(
                            "{NAME}: image attempt {attempt} failed for {candidate}: {error:#}"
                        );
                        last_error = Some(error);
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("{NAME}: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}

/// NHentai.to clips image URLs with a `#fallback[<json list>]` suffix, matching
/// the theme's OkHttp interceptor fallback chain.
fn split_image_fallbacks(url: &str) -> (String, Vec<String>) {
    if let Some((primary, fragment)) = url.split_once('#') {
        if let Some(rest) = fragment.strip_prefix("fallback") {
            let fallbacks: Vec<String> = serde_json::from_str(rest.trim()).unwrap_or_default();
            return (primary.to_string(), fallbacks);
        }
        return (primary.to_string(), Vec::new());
    }
    (url.to_string(), Vec::new())
}
