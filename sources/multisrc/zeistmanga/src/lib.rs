//! ZeistManga — Rust port of the Keiyoushi `zeistmanga` multisrc theme.
//!
//! The theme is built on Blogger: every site is a Blogger blog whose series and
//! chapters are posts tagged with labels. The port therefore:
//! - lists newest series / searches through the Blogger JSON feed
//!   (`/feeds/posts/default/?alt=json`, with a `q=label:"Series" ...` filter);
//! - reads the "popular" widget from the home page markup (sites without a
//!   popular widget fall back to the feed listing, as the Kotlin theme does);
//! - scrapes a series page for its details and finds the label its chapters are
//!   posted under (the theme's `clwd.run("label")` reader script, the newer
//!   `label = '...'` script, the old `#myUL` script, or a `data-label`
//!   attribute — configurable per site);
//! - pages the label's JSON feed for the chapter list;
//! - reads the chapter page for the page images.
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

extension_utils::export_extension!(register, ZeistManga, "ZeistManga");

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The theme requests one extra result to detect a next page.
const MAX_MANGA_RESULTS: i64 = 20;
/// Blogger ends a feed page at 150 posts or about 2 MB of content.
const MAX_CHAPTER_RESULTS: i64 = 150;

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

    #[serde(default = "default_true")]
    pub supports_latest: bool,
    #[serde(default = "default_chapter_category")]
    pub chapter_category: String,
    #[serde(default = "default_orderby")]
    pub orderby: String,
    #[serde(default)]
    pub prefer_updated: bool,
    #[serde(default)]
    pub included_categories: Option<Vec<String>>,
    #[serde(default)]
    pub excluded_categories: Option<Vec<String>>,

    // Popular widget selectors.
    #[serde(default)]
    pub popular_selector: Option<String>,
    #[serde(default)]
    pub popular_title_selector: Option<String>,
    #[serde(default)]
    pub popular_url_selector: Option<String>,

    // Detail page selectors.
    #[serde(default)]
    pub detail_selector: Option<String>,
    #[serde(default)]
    pub detail_title: Option<String>,
    #[serde(default)]
    pub detail_thumb: Option<String>,
    #[serde(default)]
    pub detail_desc: Option<String>,
    #[serde(default)]
    pub detail_genres: Option<String>,
    #[serde(default)]
    pub detail_author: Option<String>,
    #[serde(default)]
    pub detail_artist: Option<String>,
    #[serde(default)]
    pub detail_altname: Option<String>,
    #[serde(default)]
    pub detail_status: Option<String>,
    #[serde(default)]
    pub detail_info: Option<String>,
    #[serde(default)]
    pub detail_info_title: Option<String>,
    #[serde(default)]
    pub detail_info_desc: Option<String>,
    /// `reuo`: status is an optional "Completed" badge only.
    #[serde(default)]
    pub detail_status_badge: bool,

    // Chapter reading page.
    #[serde(default = "default_pages_mode")]
    pub pages_mode: String,
    #[serde(default)]
    pub pages_selector: Option<String>,
    /// Container used by the `textarea` / `raw_text` page modes.
    #[serde(default)]
    pub pages_source_selector: Option<String>,
    #[serde(default)]
    pub pages_filter_exclude: Option<String>,

    // Chapter list source.
    #[serde(default = "default_chapters_mode")]
    pub chapters_mode: String,
    #[serde(default)]
    pub chapters_selector: Option<String>,

    /// `auto`, `new`, `old`, `label_attr`, `decode_strip`, `custom_regex`.
    #[serde(default = "default_feed_mode")]
    pub chapter_feed_mode: String,
    #[serde(default)]
    pub feed_label_selector: Option<String>,
    #[serde(default)]
    pub feed_label_attr: Option<String>,
    #[serde(default)]
    pub feed_regex: Option<String>,
    #[serde(default)]
    pub feed_script_selector: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_chapter_category() -> String {
    "Chapter".to_string()
}
fn default_orderby() -> String {
    "published".to_string()
}
fn default_pages_mode() -> String {
    "html".to_string()
}
fn default_chapters_mode() -> String {
    "feed".to_string()
}
fn default_feed_mode() -> String {
    "auto".to_string()
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
        panic!("zeistmanga: sites.json is empty");
    }

    match site_key_from_args() {
        Some(key) => match sites.iter().find(|site| site.key == key) {
            Some(site) => site,
            None => {
                log::warn!(
                    "zeistmanga: no site config for plugin name '{key}', falling back to '{}'",
                    sites[0].key
                );
                &sites[0]
            }
        },
        None => {
            log::warn!(
                "zeistmanga: plugin name not found in arguments, falling back to '{}'",
                sites[0].key
            );
            &sites[0]
        }
    }
}

pub struct ZeistManga {
    site: &'static SiteConfig,
    icon: &'static str,
    client: RateLimitedAgent,
}

impl Default for ZeistManga {
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

impl ZeistManga {
    fn referer(&self) -> String {
        format!("{}/", self.site.url.trim_end_matches('/'))
    }

    pub fn base(&self) -> &str {
        self.site.url.trim_end_matches('/')
    }

    pub fn site(&self) -> &SiteConfig {
        self.site
    }

    fn absolute(&self, path: &str) -> String {
        let path = path.trim();
        if path.starts_with("http") {
            return path.to_string();
        }
        format!(
            "{}{}",
            self.base(),
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            }
        )
    }

    fn strip_base(&self, url: &str) -> String {
        url.trim()
            .strip_prefix(self.site.url.trim_end_matches('/'))
            .unwrap_or(url.trim())
            .to_string()
    }

    fn fetch_page(&self, url: &str) -> Result<Html> {
        log::debug!("zeistmanga: GET {url} ({})", self.site.key);
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        Ok(Html::parse_document(&body))
    }

    fn fetch_json(&self, url: &str) -> Result<serde_json::Value> {
        log::debug!("zeistmanga: GET feed {url} ({})", self.site.key);
        let body = self
            .client
            .fetch_text_with_headers(url, &[("Referer", &self.referer())])
            .with_context(|| format!("requesting {url}"))?;
        serde_json::from_str(&body).with_context(|| format!("decoding JSON from {url}"))
    }

    fn append_query(url: &str, query: &str) -> String {
        if query.is_empty() {
            return url.to_string();
        }
        let separator = if url.contains('?') { '&' } else { '?' };
        format!("{url}{separator}{query}")
    }

    /// Blogger feed URL for the site (`/feeds/posts/default`).
    fn api_base(&self) -> String {
        format!("{}/feeds/posts/default", self.base())
    }

    /// The label filter `q` the theme builds from its category lists.
    fn category_query(&self, extra: &str) -> String {
        let included: Vec<String> = self
            .site
            .included_categories
            .clone()
            .unwrap_or_else(|| {
                [
                    "Series",
                    "Manga",
                    "Manhwa",
                    "Manhua",
                    "Doujinshi",
                    "Web Novel (JP)",
                    "Web Novel (KR)",
                    "Web Novel (CN)",
                ]
                .iter()
                .map(|value| value.to_string())
                .collect()
            });
        let excluded: Vec<String> = self.site.excluded_categories.clone().unwrap_or_else(|| {
            vec![
                "Anime".to_string(),
                "Novel".to_string(),
                "Novela".to_string(),
                self.site.chapter_category.clone(),
            ]
        });

        let mut query = String::from("{");
        for category in &included {
            query.push_str(&format!("label:\"{category}\" "));
        }
        query.push('}');
        for category in &excluded {
            let category = category.trim();
            if !category.is_empty() {
                query.push_str(&format!("-label:\"{category}\" "));
            }
        }
        if !extra.trim().is_empty() {
            query.push_str(extra.trim());
        }
        query.trim().to_string()
    }

    /// Listing feed: newest series (latest) or the latest/search listing.
    fn listing(&self, page: i64, extra_query: &str, orderby: &str) -> Result<Vec<MangaInfo>> {
        let page = page.max(1);
        let start_index = MAX_MANGA_RESULTS * (page - 1) + 1;
        let url = Self::append_query(
            &format!("{}/", self.api_base()),
            &format!(
                "alt=json&orderby={orderby}&max-results={}&start-index={start_index}&q={}",
                MAX_MANGA_RESULTS + 1,
                urlencoding::encode(&self.category_query(extra_query))
            ),
        );
        let feed = self.fetch_json(&url)?;
        Ok(parse::feed_mangas(&feed, &self.base(), self.site.id))
    }

    fn popular(&self, page: i64) -> Result<Vec<MangaInfo>> {
        if !self.site.supports_latest {
            // The theme's fallback when a site has no popular widget.
            return self.listing(page, "", "published");
        }
        let url = format!("{}/", self.base());
        let document = self.fetch_page(&url)?;
        let mangas = parse::popular_mangas(&document, self.site, self.base());
        if mangas.is_empty() {
            log::warn!(
                "zeistmanga: popular widget empty on {}, falling back to the feed ({})",
                url,
                self.site.key
            );
            return self.listing(page, "", &self.site.orderby);
        }
        Ok(mangas)
    }

    fn detail(&self, path: &str) -> Result<MangaInfo> {
        let url = self.absolute(path);
        let document = self.fetch_page(&url)?;
        let mut manga = parse::manga_detail(&document, self.site, &url);
        manga.path = self.strip_base(&url);
        Ok(manga)
    }

    /// The title + labels of the series post, read from the Blogger feed by
    /// path. The newer layouts load their chapter list client-side, so the
    /// post's own title/label is the only reliable chapter-feed key there.
    fn post_meta(&self, url: &str) -> Option<(String, Vec<String>)> {
        let path = url.trim_start_matches(self.base());
        let feed_url = Self::append_query(
            &format!("{}/", self.api_base()),
            &format!("alt=json&path={}", urlencoding::encode(path)),
        );
        let feed = self.fetch_json(&feed_url).ok()?;
        let entry = feed.pointer("/feed/entry")?.as_array()?.first()?.clone();
        let title = parse::entry_title_text(&entry);
        let labels = parse::entry_labels(&entry);
        Some((title, labels))
    }

    /// The Blogger feed URL that lists a series' chapters.
    ///
    /// All feed generations ultimately expose chapters as posts carrying the
    /// series label, so a feed by label is used; only the legacy `#myUL` script
    /// embeds a feed URL directly.
    fn chapter_feed_url(&self, document: &Html, url: &str, manga_title: &str) -> String {
        let mode = self.site.chapter_feed_mode.as_str();
        let mut label: Option<String> = None;
        let mut direct: Option<String> = None;

        match mode {
            "old" => direct = parse::old_feed_url(document),
            "label_attr" | "custom_regex" => {
                label = parse::custom_feed_label(document, self.site);
                if label.is_none() && mode == "custom_regex" {
                    label = parse::script_feed_label(document, self.site);
                }
            }
            "auto" => {
                label = parse::script_feed_label(document, self.site);
                if label.is_none() {
                    direct = parse::old_feed_url(document);
                }
            }
            _ => label = parse::script_feed_label(document, self.site),
        }

        if label.is_none() && direct.is_none() {
            if let Some((title, labels)) = self.post_meta(url) {
                label = parse::post_label(&title, &labels, &self.site.chapter_category);
            }
        }

        if let Some(direct) = direct {
            let direct = if direct.starts_with("http") {
                direct
            } else {
                self.absolute(&direct)
            };
            let direct = direct.replace(' ', "%20");
            return if direct.contains("alt=json") {
                direct
            } else {
                Self::append_query(&direct, "alt=json")
            };
        }

        let mut label = label.unwrap_or_else(|| manga_title.to_string());
        if mode == "decode_strip" {
            label = parse::decode_label(&label);
        }
        format!(
            "{}/-/{}?alt=json",
            self.api_base(),
            label.encode_path()
        )
    }

    fn chapters(&self, path: &str) -> Result<Vec<ChapterInfo>> {
        let url = self.absolute(path);
        let document = self.fetch_page(&url)?;
        if self.site.chapters_mode == "series_list" {
            let chapters = parse::series_list_chapters(&document, self.site, self.base());
            if chapters.is_empty() {
                return Err(anyhow!("zeistmanga: no chapters found at {url} ({})", self.site.key));
            }
            return Ok(chapters);
        }

        let feed_base =
            self.chapter_feed_url(&document, &url, &parse::detail_title(&document, self.site));
        log::debug!(
            "zeistmanga: chapter feed for {} is {feed_base} ({})",
            path,
            self.site.key
        );

        let mut entries: Vec<serde_json::Value> = Vec::new();
        loop {
            let start_index = entries.len() as i64 + 1;
            let url = Self::append_query(
                &feed_base,
                &format!("start-index={start_index}&max-results={MAX_CHAPTER_RESULTS}"),
            );
            let feed = self.fetch_json(&url)?;
            let page_entries: Vec<serde_json::Value> = feed
                .pointer("/feed/entry")
                .and_then(|entries| entries.as_array())
                .cloned()
                .unwrap_or_default();
            let total = feed
                .pointer("/feed/openSearch$totalResults/$t")
                .and_then(|total| total.as_str())
                .and_then(|total| total.parse::<i64>().ok())
                .unwrap_or(0);
            if page_entries.is_empty() {
                break;
            }
            entries.extend(page_entries);
            if entries.len() as i64 >= total {
                break;
            }
        }

        let mut out: Vec<ChapterInfo> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for entry in &entries {
            if !parse::entry_has_label(entry, &self.site.chapter_category) {
                continue;
            }
            let Some((title, link)) = parse::entry_title_link(entry) else {
                continue;
            };
            let path = parse::chapter_path(&link, self.site);
            if path.is_empty() || seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            let uploaded = parse::entry_date(entry, self.site.prefer_updated);
            out.push(ChapterInfo {
                source_id: self.site.id,
                title,
                path,
                number: parse::chapter_number(&parse::entry_title_text(entry)),
                scanlator: None,
                uploaded,
            });
        }

        if out.is_empty() {
            return Err(anyhow!(
                "zeistmanga: no chapters found at {url} ({})",
                self.site.key
            ));
        }
        Ok(out)
    }

    fn pages(&self, path: &str) -> Result<Vec<String>> {
        let url = self.absolute(path);
        let document = self.fetch_page(&url)?;
        let pages = parse::chapter_pages(&document, self.site, self.base());
        if pages.is_empty() {
            return Err(anyhow!(
                "zeistmanga: no pages found at {url} ({})",
                self.site.key
            ));
        }
        Ok(pages)
    }
}

impl Extension for ZeistManga {
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
            return Err(anyhow!("zeistmanga: no popular manga at {} ({})", self.site.url, self.site.key));
        }
        Ok(list)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let list = self.listing(page, "", &self.site.orderby)?;
        if list.is_empty() {
            return Err(anyhow!("zeistmanga: no latest manga at {} ({})", self.site.url, self.site.key));
        }
        Ok(list)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let extra = if query.trim().is_empty() {
            String::new()
        } else {
            format!("{} ", query.trim())
        };
        self.listing(page, &extra, &self.site.orderby)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        self.detail(&path)
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        self.chapters(&path)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        self.pages(&path)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        log::debug!("zeistmanga: get_image_bytes url={url}");
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3u32 {
            match self
                .client
                .fetch_image_with_headers(&url, Some(&self.referer()), &[])
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::warn!(
                        "zeistmanga: image attempt {attempt} failed for {url}: {error:#} ({})",
                        self.site.key
                    );
                    last_error = Some(error);
                    if attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(750));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("zeistmanga: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}

/// Path-encode the segments of a Blogger label (spaces become `%20`).
trait EncodePath {
    fn encode_path(&self) -> String;
}

impl EncodePath for str {
    fn encode_path(&self) -> String {
        self.split('/')
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl EncodePath for String {
    fn encode_path(&self) -> String {
        self.as_str().encode_path()
    }
}
