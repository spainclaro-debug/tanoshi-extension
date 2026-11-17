//! MangaCatalog — Rust port of the Keiyoushi `mangacatalog` multisrc theme.
//!
//! The theme serves a network of single-franchise "read <title> manga online"
//! sites that share one Markdown-ish HTML layout (Tailwind-class markup).
//! Unlike the usual manga source, the site itself has no catalogue: each
//! Keiyoushi module hard-codes a `sourceList` of (title, page) pairs for its
//! franchise, so "popular"/"latest" list those entries, and "search" filters
//! that same list by title or slug. Details, chapter lists and reader pages are
//! scraped from the live site.
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

extension_utils::export_extension!(register, MangaCatalog, "MangaCatalog");

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One selectable title of a single-franchise site.
#[derive(Debug, Deserialize)]
pub struct Entry {
    pub name: String,
    /// Site-relative path (`/manga/one-piece/`) or empty for the site root.
    pub path: String,
}

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
    /// Page-layout flavour: `default`, `berserk`, `opm`, `aot`, `towerofgod`.
    #[serde(default = "default_variant")]
    pub variant: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

fn default_variant() -> String {
    "default".to_string()
}

static SITES: OnceLock<Vec<SiteConfig>> = OnceLock::new();

fn sites() -> &'static [SiteConfig] {
    SITES.get_or_init(|| {
        serde_json::from_str::<Vec<SiteConfig>>(include_str!("../sites.json"))
            .expect("sites.json is a valid site table")
    })
}

/// The process arguments. A cdylib loaded with `dlopen` gets no Rust `std` argv
/// initialisation in this host, so `std::env::args()` comes back empty: fall back
/// to `/proc/self/cmdline`, which carries the same arguments.
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

/// The site key the plugin was started as, in the documented order:
/// `TANOSHI_MULTISRC_SITE`, the `--plugin <path>` argument, then `argv[1]` when it
/// ends in `.so`. As a last resort the loaded-object map is searched for a
/// `lib<key>.so` of one of the configured sites.
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

    // Direct load by path (no `--plugin`, no argv[1]): the mapped library name
    // still names the site when it was installed as `lib<key>.so`.
    if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
        for site in sites() {
            if maps.contains(&format!("lib{}.so", site.key)) {
                return Some(site.key.clone());
            }
        }
    }

    None
}

/// The configuration for the instance this plugin is running as. Falls back to
/// the first site (with a warning) so a plain build/load smoke test still works.
fn selected_site() -> &'static SiteConfig {
    let sites = sites();
    if sites.is_empty() {
        // `sites.json` is a compile-time constant with entries; this is only a
        // guard so a malformed table cannot panic here.
        panic!("mangacatalog: sites.json is empty");
    }

    match site_key_from_args() {
        Some(key) => match sites.iter().find(|site| site.key == key) {
            Some(site) => site,
            None => {
                log::warn!(
                    "mangacatalog: no site config for plugin name '{key}', falling back to '{}'",
                    sites[0].key
                );
                &sites[0]
            }
        },
        None => {
            log::warn!(
                "mangacatalog: plugin name not found in arguments, falling back to '{}'",
                sites[0].key
            );
            &sites[0]
        }
    }
}

pub struct MangaCatalog {
    site: &'static SiteConfig,
    /// `SourceInfo.icon` is `&'static str`; leaked once per plugin instance.
    icon: &'static str,
    client: RateLimitedAgent,
}

impl Default for MangaCatalog {
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

impl MangaCatalog {
    /// Referer sent with every request: the site serves its images/pages to
    /// requests that carry the site's own origin.
    fn referer(&self) -> String {
        format!("{}/", self.site.url.trim_end_matches('/'))
    }

    /// Resolve a stored path (site-relative or absolute) to an absolute URL.
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
        log::debug!("mangacatalog: GET {url}");
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn entry_url(&self, entry: &Entry) -> String {
        self.absolute(&entry.path)
    }

    fn entry_manga(&self, entry: &Entry) -> MangaInfo {
        MangaInfo {
            source_id: self.site.id,
            title: entry.name.clone(),
            author: Vec::new(),
            genre: Vec::new(),
            status: None,
            description: None,
            path: self.entry_url(entry),
            cover_url: String::new(),
        }
    }

    /// The theme only knows a fixed list of franchise titles, so this listing is
    /// what both the popular and latest tabs show.
    fn franchise_list(&self) -> Vec<MangaInfo> {
        self.site
            .entries
            .iter()
            .map(|entry| self.entry_manga(entry))
            .collect()
    }

    fn detail(&self, path: &str) -> Result<MangaInfo> {
        let url = self.absolute(path);
        let document = self.fetch_page(&url)?;
        let mut manga = parse::manga_detail(&document, self.site, &url);
        manga.path = url;
        Ok(manga)
    }
}

impl Extension for MangaCatalog {
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

    fn get_popular_manga(&self, _page: i64) -> Result<Vec<MangaInfo>> {
        let list = self.franchise_list();
        if list.is_empty() {
            return Err(anyhow!("mangacatalog: {} has no franchise entries", self.site.key));
        }
        Ok(list)
    }

    fn get_latest_manga(&self, _page: i64) -> Result<Vec<MangaInfo>> {
        // The Keiyoushi theme declares `supportsLatest = false`, but Tanoshi asks
        // every source for a latest list. Report the site's titles unchanged.
        self.get_popular_manga(1)
    }

    fn search_manga(
        &self,
        _page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(self.franchise_list());
        }
        // Romaji titles are in the slugs, so match the slug form too.
        let slug_needle = needle.replace(' ', "-");
        let list: Vec<MangaInfo> = self
            .site
            .entries
            .iter()
            .filter(|entry| {
                entry.name.to_lowercase().contains(&needle)
                    || entry.path.to_lowercase().contains(&slug_needle)
            })
            .map(|entry| self.entry_manga(entry))
            .collect();
        Ok(list)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        self.detail(&path)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let url = self.absolute(&path);
        let document = self.fetch_page(&url)?;
        let chapters = parse::chapters(&document, self.site);
        if chapters.is_empty() {
            return Err(anyhow!(
                "mangacatalog: no chapters found at {url} ({})",
                self.site.key
            ));
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = self.absolute(&path);
        let document = self.fetch_page(&url)?;
        let pages = parse::chapter_pages(&document, self.site);
        if pages.is_empty() {
            return Err(anyhow!(
                "mangacatalog: no pages found at {url} ({})",
                self.site.key
            ));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        log::debug!("mangacatalog: get_image_bytes url={url}");
        // The franchise CDNs answer the occasional transient 403/404 for images
        // that exist (observed on readbleachmanga's CDN), so retry briefly.
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match self
                .client
                .fetch_image_with_headers(&url, Some(&self.referer()), &[])
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!(
                        "mangacatalog: image attempt {attempt} failed for {url}: {error:#} ({})",
                        self.site.key
                    );
                    last_error = Some(error);
                    if attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(750));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("mangacatalog: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}
