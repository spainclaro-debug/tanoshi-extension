//! **MangaThemesia multisrc** — one Rust plugin covering every Keiyoushi
//! source built on the shared `mangathemesia` theme (`lib-multisrc/mangathemesia`,
//! 118 catalog sources across 113 sites in the latest index).
//!
//! A Tanoshi plugin registers a single source, so the same `.so` is published
//! once per site in the extension catalog: the installed file name selects the
//! site configuration embedded in `sites.json`. The server starts extension
//! workers with `--plugin <path>`; this plugin reads its own file name
//! (`lib<key>.so` / `<key>.so`) and looks the key up in the embedded table,
//! exactly like the other multisrc plugins.
//!
//! Theme behaviour implemented here (verified against the live sites):
//! - listings are `{url}{dir}?page=N&order=popular|update` (or the search page
//!   `?title=<query>`), parsed from the `div.bsx` card grid; a few sites use
//!   their own listing mode (`catalogue`, `page_query`, `goonscans`,
//!   `advanced_search`, homepage sections);
//! - the series page carries all metadata and the chapter list inline, in
//!   `.infotable`/`.tsinfo`/`.fmed` rows and `#chapterlist`-style lists;
//! - reader pages expose `div#readerarea img`, with the `ts_reader.run(...)`
//!   JSON image array as the fallback for JS-rendered readers;
//! - covers and page images are fetched with the site root as `Referer`.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::{Html, Selector};
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, MangaThemesia, NAME);

const NAME: &str = "MangaThemesia (multisrc)";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";

fn default_lang() -> String {
    "en".to_string()
}

fn default_dir() -> String {
    "/manga".to_string()
}

fn default_true() -> bool {
    true
}

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `mangathemesia-akazascans`.
    pub key: String,
    /// Display name shown by the server, e.g. `Akaza Scans`.
    pub name: String,
    /// Site root, without a trailing slash, e.g. `https://akazascans.org`.
    pub url: String,
    /// Keiyoushi numeric source id (unique across plugins).
    pub id: i64,
    /// Keiyoushi language code (`en`, `es`, `all`, …).
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub icon: Option<String>,
    /// Whether requests send a `Referer` header (a few sites reject it).
    #[serde(default = "default_true")]
    pub referer: bool,

    /// Manga URL directory (the theme's `mangaUrlDirectory`).
    #[serde(default = "default_dir")]
    pub dir: String,
    /// Listing strategy: empty (default `?page&order`), `catalogue`,
    /// `page_query`, `goonscans`, `home`, `mangacan`, `advanced_search`.
    #[serde(default)]
    pub listing: String,
    /// Chapter list strategy: empty (default) or `reverse`.
    #[serde(default)]
    pub chapters: String,
    /// Page extraction strategy: empty (`div#readerarea img` + JSON fallback),
    /// `json` or `secure_ajax` (AreaManga-style reader API).
    #[serde(default)]
    pub pages: String,
    /// Query parameter used for text search on the listing page (default `title`).
    #[serde(default)]
    pub query_param: Option<String>,

    /// Optional chrono format for the chapter date column.
    #[serde(default)]
    pub date_format: Option<String>,

    /// Selector overrides matching the theme's per-site Kotlin overrides.
    #[serde(default)]
    pub search_sel: Option<String>,
    #[serde(default)]
    pub detail_sel: Option<String>,
    #[serde(default)]
    pub title_sel: Option<String>,
    #[serde(default)]
    pub author_sel: Option<String>,
    #[serde(default)]
    pub artist_sel: Option<String>,
    #[serde(default)]
    pub desc_sel: Option<String>,
    #[serde(default)]
    pub alt_sel: Option<String>,
    #[serde(default)]
    pub genre_sel: Option<String>,
    #[serde(default)]
    pub type_sel: Option<String>,
    #[serde(default)]
    pub status_sel: Option<String>,
    #[serde(default)]
    pub thumb_sel: Option<String>,
    #[serde(default)]
    pub chapter_sel: Option<String>,
    #[serde(default)]
    pub chapter_name_sel: Option<String>,
    #[serde(default)]
    pub chapter_date_sel: Option<String>,
    #[serde(default)]
    pub page_sel: Option<String>,

    /// Extra image source attributes to prefer, in order.
    #[serde(default)]
    pub img_attrs: Vec<String>,
    /// Rewrite page-image hosts that the site still emits for images that now
    /// live on the site's current domain, e.g. `miauscans.com` -> current host.
    #[serde(default)]
    pub page_hosts: Vec<(String, String)>,
    /// Strip a leading parenthesised language marker from titles
    /// (MiauScan publishes e.g. `(Português) Title`).
    #[serde(default)]
    pub strip_paren_prefix: bool,
    /// Heading used when appending alternative names to the description.
    #[serde(default)]
    pub alt_prefix: Option<String>,
    /// Extra query parameters appended to every listing request.
    #[serde(default)]
    pub extra_query: Vec<(String, String)>,
}

impl Site {
    fn base(&self) -> String {
        self.url.trim_end_matches('/').to_string()
    }

    fn referer(&self) -> String {
        format!("{}/", self.base())
    }

    fn icon_url(&self) -> String {
        self.icon
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("{}/favicon.ico", self.base()))
    }

    /// `{base}{dir}` with the directory normalised to a leading slash.
    fn dir_path(&self) -> String {
        let dir = self.dir.trim_matches('/');
        if dir.is_empty() {
            self.base()
        } else {
            format!("{}/{}", self.base(), dir)
        }
    }

    fn absolute(&self, reference: &str) -> String {
        let reference = reference.trim();
        if reference.starts_with("http://") || reference.starts_with("https://") {
            reference.to_string()
        } else if let Some(rest) = reference.strip_prefix("//") {
            format!("https://{rest}")
        } else if reference.starts_with('/') {
            format!("{}{}", self.base(), reference)
        } else {
            format!("{}/{}", self.base(), reference)
        }
    }

    /// Stored identity for a link: a root-relative path when it lives on the
    /// configured site, the absolute URL otherwise (foreign reader hosts).
    fn stored_identity(&self, href: &str) -> Option<String> {
        let absolute = self.absolute(href);
        if !absolute.starts_with("http") {
            return None;
        }
        let base = self.base();
        let mut identity = if absolute.starts_with(&base) {
            absolute[base.len()..].to_string()
        } else {
            return Some(absolute);
        };
        if let Some(position) = identity.find(['?', '#']) {
            identity.truncate(position);
        }
        if identity.is_empty() {
            return None;
        }
        if !identity.starts_with('/') {
            identity.insert(0, '/');
        }
        Some(identity)
    }

    /// Request URL for a stored identity (path or absolute URL).
    fn request_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{}", self.base(), path)
        } else {
            format!("{}/{}", self.base(), path)
        }
    }

    fn search_selector(&self) -> &str {
        self.search_sel
            .as_deref()
            .unwrap_or(parse::DEFAULT_SEARCH_SELECTOR)
    }

    fn query_param(&self) -> &str {
        self.query_param.as_deref().unwrap_or("title")
    }

    /// Standard listing URL: `{dir}?title=<q>&page=N&order=<order>`.
    fn listing_url(&self, page: i64, order: &str) -> String {
        let page = page.max(1);
        let mut url = self.dir_path();
        url.push('?');
        let mut pushed = false;
        for (name, value) in &self.extra_query {
            if !value.is_empty() {
                url.push_str(&format!("{name}={}&", urlencoding::encode(value)));
                pushed = true;
            }
        }
        let _ = pushed;
        url.push_str(&format!("page={page}"));
        if !order.is_empty() {
            url.push_str("&order=");
            url.push_str(order);
        }
        url
    }

    fn search_url(&self, page: i64, query: &str) -> String {
        let page = page.max(1);
        let mut url = self.dir_path();
        url.push('?');
        if !query.is_empty() {
            url.push_str(self.query_param());
            url.push('=');
            url.push_str(&urlencoding::encode(query));
            url.push('&');
        }
        for (name, value) in &self.extra_query {
            if !value.is_empty() {
                url.push_str(&format!("{name}={}&", urlencoding::encode(value)));
            }
        }
        url.push_str(&format!("page={page}"));
        url
    }
}

static SITES: OnceLock<Vec<Site>> = OnceLock::new();

fn sites() -> &'static [Site] {
    SITES.get_or_init(|| {
        serde_json::from_str(include_str!("../sites.json"))
            .expect("embedded sites.json must be valid")
    })
}

/// Site key encoded in a plugin file name (`lib<key>.so` / `<key>.so`).
fn key_from_path(path: &str) -> Option<String> {
    let file = std::path::Path::new(path)
        .file_name()?
        .to_string_lossy()
        .to_string();
    let stem = file
        .strip_suffix(".so")
        .or_else(|| file.strip_suffix(".SO"))
        .unwrap_or(&file);
    let key = stem.strip_prefix("lib").unwrap_or(stem);
    (!key.is_empty()).then(|| key.to_string())
}

fn key_from_args(args: &[String]) -> Option<String> {
    let path = args
        .iter()
        .position(|arg| arg == "--plugin")
        .and_then(|index| args.get(index + 1))
        .or_else(|| args.iter().skip(1).rev().find(|arg| arg.ends_with(".so")))
        .or_else(|| args.iter().find(|arg| arg.ends_with(".so")))?;
    key_from_path(path)
}

/// A cdylib loaded through `dlopen` sees an empty `std::env::args()`, so read
/// the real command line from `/proc/self/cmdline` (NUL separated).
fn argv_from_proc() -> Vec<String> {
    let Ok(raw) = std::fs::read("/proc/self/cmdline") else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect()
}

/// Last resort: find the loaded plugin file name in `/proc/self/maps`.
fn key_from_maps() -> Option<String> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().last() else {
            continue;
        };
        if !path.ends_with(".so") {
            continue;
        }
        if let Some(key) = key_from_path(path) {
            if sites().iter().any(|site| site.key == key) {
                return Some(key);
            }
        }
    }
    None
}

/// Key of the installed plugin: env var for tests, else `--plugin <path>` /
/// argv, else the loaded `.so` path from `/proc`.
fn selected_key() -> Option<String> {
    if let Ok(key) = std::env::var("TANOSHI_MULTISRC_SITE") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }

    let args: Vec<String> = std::env::args().collect();
    if let Some(key) = key_from_args(&args) {
        return Some(key);
    }

    let proc_args = argv_from_proc();
    log::debug!("{NAME}: process arguments {:?}", proc_args);
    if let Some(key) = key_from_args(&proc_args) {
        return Some(key);
    }

    key_from_maps()
}

fn selected_site() -> Site {
    let fallback = || {
        let site = sites()
            .first()
            .cloned()
            .expect("sites.json must not be empty");
        log::warn!("{NAME}: no site key detected, using {}", site.key);
        site
    };
    let Some(key) = selected_key() else {
        return fallback();
    };
    match sites().iter().find(|site| site.key == key) {
        Some(site) => site.clone(),
        None => {
            let site = sites()
                .first()
                .cloned()
                .expect("sites.json must not be empty");
            log::warn!("{NAME}: unknown site key {key}, using {}", site.key);
            site
        }
    }
}

pub struct MangaThemesia {
    site: Site,
    icon: &'static str,
    client: RateLimitedAgent,
}

impl Default for MangaThemesia {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        let icon: &'static str = Box::leak(site.icon_url().into_boxed_str());
        Self {
            site,
            icon,
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), None),
        }
    }
}

impl MangaThemesia {
    fn request_headers(&self) -> Vec<(&'static str, String)> {
        if self.site.referer {
            vec![("Referer", self.site.referer())]
        } else {
            Vec::new()
        }
    }

    fn get(&self, url: &str) -> Result<Html> {
        log::debug!("{NAME}: GET {url}");
        let headers = self.request_headers();
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let body = self
            .client
            .fetch_text_with_headers(url, &borrowed)
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        Ok(Html::parse_document(&body))
    }

    /// One listing page. `order` is `popular` or `update` for the theme's
    /// listings; the search variants pass the query instead.
    fn listing(&self, page: i64, order: &str) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let url = match self.site.listing.as_str() {
            "catalogue" => format!("{}/?page={page}&order={order}", self.site.dir_path()),
            "page_query" => format!("{}/page/{page}?s=&order={order}", self.site.base()),
            "goonscans" => {
                let order = if order == "popular" {
                    "top_rated"
                } else {
                    "latest_chapter"
                };
                let mut url = format!("{}/title", self.site.base());
                if page > 1 {
                    url.push_str(&format!("/page/{page}"));
                }
                url.push_str(&format!("/?orderby={order}"));
                url
            }
            "advanced_search" => format!(
                "{}/advanced-search?name=&page={page}&sort={order}",
                self.site.base()
            ),
            "home" => return self.home_listing(page, order),
            "mangacan" => self.site.base(),
            _ => self.site.listing_url(page, order),
        };

        let doc = self.get(&url)?;
        let cards = parse::manga_cards(&self.site, &doc, self.site.search_selector());
        if cards.is_empty() && self.site.listing == "home" {
            return Ok(cards);
        }
        Ok(cards)
    }

    /// Homepage-section listing used by sites whose archive is the front page.
    fn home_listing(&self, page: i64, kind: &str) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let url = if page <= 1 {
            self.site.base()
        } else {
            format!("{}/page/{page}/", self.site.base())
        };
        let doc = self.get(&url)?;
        let root = doc.root_element();

        let mut out: Vec<MangaInfo> = Vec::new();
        let boxes = Selector::parse("div.bixbox, div.listupd").expect("static selector");
        for section in root.select(&boxes) {
            let heading = section
                .select(&Selector::parse("h2, h1, .heading").expect("static"))
                .next()
                .map(|element| {
                    element
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase()
                })
                .unwrap_or_default();
            if !heading.contains(kind) {
                continue;
            }
            let cards = parse::manga_cards_in(&self.site, &section, ".bsx, .bs .bsx, .uta");
            if !cards.is_empty() {
                out.extend(cards);
            }
        }

        if out.is_empty() {
            out = parse::manga_cards(&self.site, &doc, self.site.search_selector());
        }
        Ok(out)
    }

    /// Text search page.
    fn search(&self, page: i64, query: &str) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let query = query.trim();
        let url = match self.site.listing.as_str() {
            "catalogue" | "page_query" => {
                format!("{}/page/{page}?s={}", self.site.base(), urlencoding::encode(query))
            }
            "goonscans" => {
                if query.is_empty() {
                    self.site.listing_url(page, "update")
                } else {
                    // The theme search path 404s on the live site; the series
                    // index exposes a working `search` parameter instead.
                    format!(
                        "{}/title/?search={}",
                        self.site.base(),
                        urlencoding::encode(query)
                    )
                }
            }
            "advanced_search" => format!(
                "{}/advanced-search?name={}&page={page}&sort=",
                self.site.base(),
                urlencoding::encode(query)
            ),
            "mangacan" => {
                let slug = query
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-")
                    .to_lowercase();
                format!("{}/cari/{}/{page}.html", self.site.base(), urlencoding::encode(&slug))
            }
            "home" => format!(
                "{}/?s={}&page={page}",
                self.site.base(),
                urlencoding::encode(query)
            ),
            _ => self.site.search_url(page, query),
        };

        let doc = self.get(&url)?;
        Ok(parse::manga_cards(
            &self.site,
            &doc,
            self.site.search_selector(),
        ))
    }

    fn detail_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        let trimmed = path.trim();
        if trimmed.starts_with('/') {
            self.site.request_url(trimmed)
        } else if trimmed.is_empty() {
            format!("{}/", self.site.base())
        } else {
            // Bare slug (older catalogues) -> rebuild the series URL.
            let dir = self.site.dir.trim_matches('/');
            if dir.is_empty() {
                format!("{}/{}/", self.site.base(), trimmed)
            } else {
                format!("{}/{}/{}/", self.site.base(), dir, trimmed)
            }
        }
    }

    /// Reader pages for AreaManga-style sites that decode the image list via
    /// `admin-ajax.php?action=get_secure_chapter_images`.
    fn secure_ajax_pages(&self, doc: &Html) -> Result<Vec<String>> {
        let root = doc.root_element();
        let chapter_id = root
            .select(&Selector::parse("#comment_post_ID").expect("static"))
            .next()
            .and_then(|element| element.value().attr("value").map(str::trim))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("{}: chapter id not found", self.site.key))?
            .to_string();

        let url = format!("{}/wp-admin/admin-ajax.php", self.site.base());
        log::debug!("{NAME}: POST {url} (chapter {chapter_id})");
        let headers = self.request_headers();
        let mut header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        header_refs.push(("X-Requested-With", "XMLHttpRequest"));
        let (body, _cookies) = self
            .client
            .post_form_with_headers(
                &url,
                &[
                    ("action", "get_secure_chapter_images"),
                    ("chapter_id", chapter_id.as_str()),
                ],
                &header_refs,
            )
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;

        #[derive(Deserialize)]
        struct SecureResponse {
            success: bool,
            data: SecureData,
        }
        #[derive(Deserialize)]
        struct SecureData {
            status: String,
            content: Option<String>,
        }

        let parsed: SecureResponse = serde_json::from_str(&body)
            .with_context(|| format!("{}: invalid secure chapter response", self.site.key))?;
        if !parsed.success {
            return Err(anyhow!("{}: secure chapter request failed", self.site.key));
        }
        if parsed.data.status != "unlocked" {
            return Err(anyhow!(
                "{}: chapter is {} (needs a website unlock)",
                self.site.key,
                parsed.data.status
            ));
        }

        let html = parsed
            .data
            .content
            .ok_or_else(|| anyhow!("{}: secure chapter content missing", self.site.key))?;
        let fragment = Html::parse_fragment(&html);
        let root = fragment.root_element();
        let mut out = Vec::new();
        for img in root.select(&Selector::parse("img").expect("static")) {
            if let Some(src) = parse::image_src(&self.site, &img) {
                if !src.starts_with("data:") {
                    out.push(src);
                }
            }
        }
        Ok(out)
    }
}

impl Extension for MangaThemesia {
    fn get_source_info(&self) -> SourceInfo {
        let languages = if self.site.lang.is_empty() || self.site.lang == "all" {
            Lang::All
        } else {
            Lang::Single(self.site.lang.clone())
        };
        SourceInfo {
            id: self.site.id,
            name: self.site.name.clone(),
            url: self.site.base(),
            version: VERSION,
            icon: self.icon,
            languages,
            nsfw: self.site.nsfw,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.listing(page, "popular")
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.listing(page, "update")
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        self.search(page, &query.unwrap_or_default())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let url = self.detail_url(&path);
        let doc = self.get(&url)?;
        let stored = if path.starts_with("http") || path.starts_with('/') {
            path
        } else {
            let dir = self.site.dir.trim_matches('/');
            if dir.is_empty() {
                format!("/{}/", path.trim_matches('/'))
            } else {
                format!("/{}/", format!("{dir}/{}", path.trim_matches('/')))
            }
        };
        Ok(parse::manga_detail(&self.site, &doc, &stored))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let url = self.detail_url(&path);
        let doc = self.get(&url)?;
        Ok(parse::chapter_list(&self.site, &doc))
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = self.site.request_url(&path);
        let doc = self.get(&url)?;
        let pages = match self.site.pages.as_str() {
            "json" => parse::page_list_from_scripts(&self.site, &doc),
            "secure_ajax" => self.secure_ajax_pages(&doc)?,
            _ => parse::page_list(&self.site, &doc)?,
        };
        // Some readers carry absolute URLs without the scheme, and some
        // sites ship the now-dead `i.ibb.co.com` host for imgbb images.
        let pages: Vec<String> = pages
            .into_iter()
            .map(|page| {
                let page = if page.starts_with("//") {
                    format!("https:{page}")
                } else {
                    page
                };
                let mut page = page
                    .replace("//i.ibb.co.com/", "//i.ibb.co/")
                    .replace("//ibb.co.com/", "//ibb.co/");
                for (from, to) in &self.site.page_hosts {
                    if page.contains(from.as_str()) {
                        page = page.replace(from.as_str(), to.as_str());
                    }
                }
                page
            })
            .filter(|page| page.starts_with("http"))
            .collect();
        if pages.is_empty() {
            return Err(anyhow!("{}: no pages found at {url}", self.site.key));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let referer = if self.site.referer {
            Some(self.site.referer())
        } else {
            None
        };
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match self.client.fetch_bytes(&url, referer.as_deref()) {
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
