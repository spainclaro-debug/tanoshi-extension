//! Keyoapp — Rust port of the Keiyoushi `keyoapp` multisrc theme.
//!
//! Keyoapp is a paid-comic platform deployed with several layout generations.
//! The Keiyoushi theme targets the classic layout (`Keyoapp`) and a newer
//! Payload-style one (`KeyoappV2`); the live sites also ship a third
//! "immersive reader" layout (per-site Kotlin overrides use `/search` and
//! `[wire:key*=serie]` cards). This port covers the classic and immersive
//! layouts, selected per site from `sites.json`.
//!
//! One shared `.so` serves every site: the plugin picks its configuration from
//! its own file name (see [`selected_site`]).

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use scraper::Html;
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod parse;

extension_utils::export_extension!(register, Keyoapp, "Keyoapp");

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One site configuration row from `sites.json`.
#[derive(Debug, Deserialize)]
pub struct SiteConfig {
    pub key: String,
    pub name: String,
    pub url: String,
    pub id: i64,
    pub lang: String,
    pub nsfw: bool,
    pub icon: String,
    /// `classic` (Keyoapp) or `wire` (immersive-reader redesign).
    #[serde(default = "default_variant")]
    pub variant: String,
    /// Listing flavour, defaults follow from `variant`.
    #[serde(default)]
    pub popular_mode: Option<String>,
    #[serde(default)]
    pub latest_mode: Option<String>,
    #[serde(default)]
    pub search_mode: Option<String>,
    /// Detail/chapter/page parsers, defaults follow from `variant`.
    #[serde(default)]
    pub detail_mode: Option<String>,
    #[serde(default)]
    pub chapters_mode: Option<String>,
    #[serde(default)]
    pub pages_mode: Option<String>,
}

fn default_variant() -> String {
    "classic".to_string()
}

impl SiteConfig {
    pub fn popular_mode(&self) -> &str {
        self.popular_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire_search",
            _ => "trending",
        })
    }

    pub fn latest_mode(&self) -> &str {
        self.latest_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire_latest",
            _ => "grid",
        })
    }

    pub fn search_mode(&self) -> &str {
        self.search_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire_search",
            _ => "series_ssr",
        })
    }

    pub fn detail_mode(&self) -> &str {
        self.detail_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire",
            _ => "classic",
        })
    }

    pub fn chapters_mode(&self) -> &str {
        self.chapters_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire",
            _ => "classic",
        })
    }

    pub fn pages_mode(&self) -> &str {
        self.pages_mode.as_deref().unwrap_or(match self.variant.as_str() {
            "wire" => "wire",
            _ => "classic",
        })
    }
}

static SITES: OnceLock<Vec<SiteConfig>> = OnceLock::new();

fn sites() -> &'static [SiteConfig] {
    SITES.get_or_init(|| {
        serde_json::from_str::<Vec<SiteConfig>>(include_str!("../sites.json"))
            .expect("sites.json is a valid site table")
    })
}

/// The process arguments. A cdylib loaded with `dlopen` gets no Rust `std` argv
/// initialisation in this host, so `std::env::args()` comes back empty: fall
/// back to `/proc/self/cmdline`, which carries the same arguments.
fn process_args() -> Vec<String> {
    let args: Vec<String> = std::env::args().collect();
    if !args.is_empty() {
        return args;
    }
    std::fs::read("/proc/self/cmdline")
        .map(|raw| {
            raw.split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The site key this plugin runs as, in the documented order:
/// `TANOSHI_MULTISRC_SITE`, `--plugin <path>`, `argv[1]` ending in `.so`, then
/// the mapped `lib<key>.so` name as a last resort.
fn site_key_from_args() -> Option<String> {
    if let Ok(key) = std::env::var("TANOSHI_MULTISRC_SITE") {
        let key = key.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }

    let args = process_args();
    let mut plugin_path: Option<&str> = None;
    if let Some(position) = args.iter().position(|argument| argument == "--plugin") {
        plugin_path = args.get(position + 1).map(String::as_str);
    }
    if plugin_path.is_none() {
        if let Some(argument) = args.get(1) {
            if argument.ends_with(".so") {
                plugin_path = Some(argument);
            }
        }
    }

    if let Some(path) = plugin_path {
        let stem = std::path::Path::new(path)
            .file_stem()?
            .to_string_lossy()
            .into_owned();
        return Some(stem.strip_prefix("lib").unwrap_or(&stem).to_string());
    }

    if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
        for site in sites() {
            if maps.contains(&format!("lib{}.so", site.key)) {
                return Some(site.key.clone());
            }
        }
    }

    None
}

fn selected_site() -> &'static SiteConfig {
    let sites = sites();
    if sites.is_empty() {
        panic!("keyoapp: sites.json is empty");
    }

    match site_key_from_args() {
        Some(key) => match sites.iter().find(|site| site.key == key) {
            Some(site) => site,
            None => {
                log::warn!(
                    "keyoapp: no site config for plugin name '{key}', falling back to '{}'",
                    sites[0].key
                );
                &sites[0]
            }
        },
        None => {
            log::warn!(
                "keyoapp: plugin name not found in arguments, falling back to '{}'",
                sites[0].key
            );
            &sites[0]
        }
    }
}

pub struct Keyoapp {
    site: &'static SiteConfig,
    icon: &'static str,
    client: RateLimitedAgent,
}

impl Default for Keyoapp {
    fn default() -> Self {
        let site = selected_site();
        Self {
            site,
            icon: Box::leak(site.icon.clone().into_boxed_str()),
            client: build_rate_limited_ureq_agent(
                Some(concat!("Tanoshi-Extension/", env!("CARGO_PKG_VERSION"))),
                None,
            ),
        }
    }
}

impl Keyoapp {
    fn referer(&self) -> String {
        format!("{}/", self.site.url.trim_end_matches('/'))
    }

    fn absolute(&self, path: &str) -> String {
        let path = path.trim();
        if path.starts_with("http") {
            return path.to_string();
        }
        format!(
            "{}{}",
            self.site.url.trim_end_matches('/'),
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            }
        )
    }

    fn fetch_page(&self, url: &str) -> Result<Html> {
        log::debug!("keyoapp: GET {url} ({})", self.site.key);
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn fetch_fragment(&self, url: &str) -> Result<Html> {
        log::debug!("keyoapp: GET fragment {url} ({})", self.site.key);
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_fragment(&body))
    }

    fn fetch_json(&self, url: &str) -> Result<serde_json::Value> {
        log::debug!("keyoapp: GET json {url} ({})", self.site.key);
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        serde_json::from_str(&body).with_context(|| format!("decoding JSON from {url}"))
    }

    pub fn base(&self) -> &str {
        self.site.url.trim_end_matches('/')
    }

    pub fn site(&self) -> &SiteConfig {
        self.site
    }

    fn popular(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let base = self.base().to_string();
        match self.site.popular_mode() {
            "wire_search" => {
                let url = format!("{base}/search?sort=popular&page={page}");
                let document = self.fetch_page(&url)?;
                Ok(parse::wire_cards(&document, self.site.id, &base))
            }
            "search_page" => {
                // The site lists its whole catalogue on the series page.
                let url = format!("{base}/series");
                let document = self.fetch_page(&url)?;
                Ok(parse::searched_cards(&document, self.site.id, &base))
            }
            "splide" => {
                let url = format!("{base}/");
                let document = self.fetch_page(&url)?;
                Ok(parse::splide_cards(&document, self.site.id, &base))
            }
            "trending2" => {
                let url = format!("{base}/");
                let document = self.fetch_page(&url)?;
                Ok(parse::trending_cards(&document, self.site.id, &base, ".group", true))
            }
            "trending3" => {
                let url = format!("{base}/");
                let document = self.fetch_page(&url)?;
                Ok(parse::trending_cards(
                    &document,
                    self.site.id,
                    &base,
                    ".group.overflow-hidden",
                    false,
                ))
            }
            _ => {
                let url = format!("{base}/");
                let document = self.fetch_page(&url)?;
                Ok(parse::trending_cards(
                    &document,
                    self.site.id,
                    &base,
                    ".group.overflow-hidden.grid",
                    false,
                ))
            }
        }
    }

    fn latest(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let base = self.base().to_string();
        match self.site.latest_mode() {
            "wire_latest" => {
                let url = format!("{base}/latest?page={page}");
                let document = self.fetch_page(&url)?;
                Ok(parse::wire_latest_cards(&document, self.site.id, &base))
            }
            "latest_poster" => {
                let url = format!("{base}/latest/?page={page}");
                let document = self.fetch_page(&url)?;
                Ok(parse::latest_cards(
                    &document,
                    self.site.id,
                    &base,
                    "div.grid > div.group.latest-poster",
                ))
            }
            _ => {
                let url = format!("{base}/latest/?page={page}");
                let document = self.fetch_page(&url)?;
                Ok(parse::latest_cards(
                    &document,
                    self.site.id,
                    &base,
                    "div.grid > div.group",
                ))
            }
        }
    }

    fn search(&self, page: i64, query: &str) -> Result<Vec<MangaInfo>> {
        let query = query.trim();
        let page = page.max(1);
        let base = self.base().to_string();
        match self.site.search_mode() {
            "wire_search" => {
                let mut url = format!("{base}/search?title={}", urlencoding::encode(query));
                if page > 1 {
                    url.push_str(&format!("&page={page}"));
                }
                let document = self.fetch_page(&url)?;
                Ok(parse::wire_cards(&document, self.site.id, &base))
            }
            _ => {
                let url = if query.is_empty() {
                    format!("{base}/series")
                } else {
                    format!("{base}/series?q={}", urlencoding::encode(query))
                };
                let document = self.fetch_page(&url)?;
                let mut cards = parse::searched_cards(&document, self.site.id, &base);
                if cards.is_empty() && !query.is_empty() {
                    // Some deployments render the result grid client-side from
                    // this endpoint instead of server-rendering it.
                    let ajax = format!(
                        "{base}/pages/series-results.php?q={}&page={page}",
                        urlencoding::encode(query)
                    );
                    if let Ok(fragment) = self.fetch_fragment(&ajax) {
                        cards = parse::searched_cards(&fragment, self.site.id, &base);
                    }
                }
                // The classic theme filters the result grid client-side by the
                // `title` attribute (some deployments ignore `?q=`).
                Ok(parse::filter_by_query(cards, query))
            }
        }
    }
}

impl Extension for Keyoapp {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: self.site.id,
            name: self.site.name.clone(),
            url: self.site.url.clone(),
            version: VERSION,
            icon: self.icon,
            languages: Lang::Single(self.site.lang.clone()),
            nsfw: self.site.nsfw,
        }
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let list = self.popular(page)?;
        if list.is_empty() {
            return Err(anyhow!("keyoapp: no popular manga at {} ({})", self.site.url, self.site.key));
        }
        Ok(list)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let list = self.latest(page)?;
        if list.is_empty() {
            return Err(anyhow!("keyoapp: no latest manga at {} ({})", self.site.url, self.site.key));
        }
        Ok(list)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        self.search(page, query.unwrap_or_default().as_str())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let url = self.absolute(&path);
        let document = self.fetch_page(&url)?;
        let mut manga = parse::manga_detail(&document, self.site, &url);
        manga.path = url;
        Ok(manga)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let url = self.absolute(&path);
        let document = self.fetch_page(&url)?;
        let chapters = parse::chapters(&document, self.site);
        if chapters.is_empty() {
            return Err(anyhow!(
                "keyoapp: no chapters found at {url} ({})",
                self.site.key
            ));
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = self.absolute(&path);
        let document = self.fetch_page(&url)?;
        let pages = parse::pages(&document, self.site, &url)?;
        if pages.is_empty() {
            return Err(anyhow!(
                "keyoapp: no pages found at {url} ({}) — the chapter may be locked",
                self.site.key
            ));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        log::debug!("keyoapp: get_image_bytes url={url}");
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match self
                .client
                .fetch_image_with_headers(&url, Some(&self.referer()), &[])
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!(
                        "keyoapp: image attempt {attempt} failed for {url}: {error:#} ({})",
                        self.site.key
                    );
                    last_error = Some(error);
                    if attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(750));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("keyoapp: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}
