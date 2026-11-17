//! **HentaiHand multisrc** — one Rust plugin covering every Keiyoushi
//! `hentaihand`-theme source (`lib-multisrc/hentaihand`): HentaiHand
//! (`hentaihand.com`) and nHentai.com (`nhentai.com`), for every language the
//! latest index publishes.
//!
//! A Tanoshi plugin registers a single source, so this binary is published once
//! per site in the extension catalog: every catalog entry points at the same
//! `.so` and the installed file name selects the site/source configuration
//! embedded in [`sites.json`](../sites.json). On Android/Termux the server
//! starts the worker with `--plugin <path>`; `std::env::args()` is empty inside
//! a `dlopen`ed library, so the plugin reads `/proc/self/cmdline`, falls back to
//! `dladdr` on its own code, and finally to `TANOSHI_MULTISRC_SITE` (tests).
//!
//! Both sites share one JSON API:
//! - listings/search: `GET /api/comics` (paged, `sort`/`order`/`q`, optional
//!   `languages[-1]=<id>` filters);
//! - details: `GET /api/comics/<slug>` (single chapter is embedded in the same
//!   payload — the theme's `chapters == false` path);
//! - pages: `GET /api/comics/<slug>/images` -> `images[].source_url`;
//! - images: plain CDN GET (`cdn.hentaihand.com` / `cdn.nhentai.com`).
//!
//! Verified against both live sites.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

extension_utils::export_extension!(register, HentaiHand, NAME);

const NAME: &str = "HentaiHand";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `hentaihand-hentaihand-en`.
    pub key: String,
    /// Display name shown by the server, e.g. `HentaiHand`.
    pub name: String,
    /// Site root, no trailing slash, e.g. `https://hentaihand.com`.
    pub url: String,
    /// Keiyoushi numeric source id (unique across plugins).
    pub id: i64,
    /// Keiyoushi language code (`en`, `all`, `pt-BR`, …).
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub icon: String,
    /// Theme site slug (`hentaihand` or `nhentaicom`).
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

/// Language ids understood by the API for this site, resolved from
/// `/api/languages` at runtime.
///
/// The Kotlin theme hard-codes legacy ids that no longer match the live API
/// (the current sites publish 22 languages on two pages), so the ids are
/// resolved by slug instead.
fn lang_slug(code: &str) -> Option<&'static str> {
    Some(match code {
        "zh" => "chinese",
        "en" => "english",
        "ja" => "japanese",
        "other" => "text-cleaned",
        "ar" => "arabic",
        "jv" => "javanese",
        "bg" => "bulgarian",
        "cs" => "czech",
        "uk" => "ukrainian",
        "sk" => "slovak",
        "eo" => "esperanto",
        "mn" => "mongolian",
        "la" => "latin",
        "ceb" => "cebuano",
        "tl" => "tagalog",
        "fi" => "finnish",
        "tr" => "turkish",
        "sr" => "serbian",
        "el" => "greek",
        "ko" => "korean",
        "ro" => "romanian",
        "pt-BR" => "portuguese-br",
        "pt" => "portuguese",
        "fr" => "french",
        "de" => "german",
        "es" => "spanish",
        "it" => "italian",
        "ru" => "russian",
        "pl" => "polish",
        "nl" => "dutch",
        "hu" => "hungarian",
        "id" => "indonesian",
        "vi" => "vietnamese",
        "th" => "thai",
        "hi" => "hindi",
        _ => return None,
    })
}

/* ------------------------------- API DTOs ------------------------------- */

#[derive(Debug, Deserialize)]
struct ResponseDto<T> {
    data: T,
    #[serde(default)]
    next_page_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ComicDto {
    slug: String,
    title: String,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    thumb_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NameDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ComicDetailDto {
    title: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    thumb_url: Option<String>,
    #[serde(default)]
    artists: Option<Vec<NameDto>>,
    #[serde(default)]
    authors: Option<Vec<NameDto>>,
    #[serde(default)]
    tags: Option<Vec<NameDto>>,
    #[serde(default)]
    relationships: Option<Vec<NameDto>>,
    #[serde(default)]
    groups: Option<Vec<NameDto>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    alternative_title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pages: Option<i64>,
    #[serde(default)]
    category: Option<NameDto>,
    #[serde(default)]
    language: Option<NameDto>,
    #[serde(default)]
    parodies: Option<Vec<NameDto>>,
    #[serde(default)]
    characters: Option<Vec<NameDto>>,
}

#[derive(Debug, Deserialize)]
struct ImagesDto {
    #[serde(default)]
    images: Vec<ImageDto>,
}

#[derive(Debug, Deserialize)]
struct ImageDto {
    page: i64,
    source_url: String,
}

/// Trimmed names joined with `, ` like the Kotlin `List<NameDto>.toNames()`.
fn names(values: &Option<Vec<NameDto>>) -> Option<String> {
    let values = values.as_ref()?;
    if values.is_empty() {
        return None;
    }
    Some(
        values
            .iter()
            .map(|value| value.name.trim())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// `manga.url` in the theme is `/en/comic/<slug>`.
fn path_for(slug: &str) -> String {
    format!("/en/comic/{slug}")
}

fn slug_from_path(path: &str) -> String {
    let path = path.trim();
    if let Some(rest) = path.split("/en/comic/").nth(1) {
        return rest.trim_matches('/').to_string();
    }
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

pub struct HentaiHand {
    site: Site,
    client: RateLimitedAgent,
    /// Cached `/api/languages` table (`slug`, `id`).
    languages: std::sync::Mutex<Option<Vec<(String, i64)>>>,
}

impl Default for HentaiHand {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        // nhentai.com sits behind Cloudflare bot management, which answers
        // HTTP 403 to a browser User-Agent sent over a non-browser TLS
        // fingerprint. The theme's own OkHttp client is not affected, but our
        // HTTP stack is, so that host identifies honestly instead.
        let user_agent = if site.slug == "nhentaicom" {
            None
        } else {
            Some(USER_AGENT)
        };
        Self {
            site,
            client: build_rate_limited_ureq_agent(user_agent, None),
            languages: std::sync::Mutex::new(None),
        }
    }
}

impl HentaiHand {
    fn api_get(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.site.base(), path);
        log::debug!("{NAME}: GET {url}");
        let referer = self.site.referer();
        let headers = [("Accept", "application/json"), ("Referer", referer.as_str())];
        self.client
            .fetch_text_with_headers(&url, &headers)
            .with_context(|| format!("{}: requesting {url}", self.site.key))
    }

    fn comic_url(
        &self,
        page: i64,
        mode: &str,
        query: Option<&str>,
        include_languages: bool,
    ) -> Result<String> {
        let mut path = format!("/api/comics?page={page}");
        match mode {
            "popular" => path.push_str("&sort=popularity&order=desc&duration=all"),
            "latest" => path.push_str("&sort=uploaded_at&order=desc&duration=all"),
            _ => {
                path.push_str("&sort=uploaded_at&order=desc&duration=all");
                if let Some(query) = query.filter(|query| !query.is_empty()) {
                    if let Some((uri, id)) = self.lookup_filter_id(query) {
                        path.push_str(&format!("&{uri}%5B0%5D={id}"));
                    } else {
                        path.push_str("&q=");
                        path.push_str(&urlencoding::encode(query));
                    }
                }
            }
        }
        if include_languages {
            for (index, id) in self.resolved_lang_ids().iter().enumerate() {
                path.push_str(&format!(
                    "&languages%5B{}%5D={id}",
                    -(index as i64) - 1
                ));
            }
        }
        Ok(path)
    }

    /// Resolve a plain-text query to a tag/artist/character id (exact match
    /// first, in the theme's lookup order), mirroring the Kotlin theme so that
    /// a text search returns the same results.
    fn lookup_filter_id(&self, query: &str) -> Option<(&'static str, i64)> {
        for uri in ["tags", "artists", "characters"] {
            let path = format!("/api/{uri}?q={}", urlencoding::encode(query));
            let body = match self.api_get(&path) {
                Ok(body) => body,
                Err(error) => {
                    log::debug!("{NAME}: lookup {uri} failed: {error:#}");
                    continue;
                }
            };
            let results: ResponseDto<Vec<LookupDto>> = match serde_json::from_str(&body) {
                Ok(results) => results,
                Err(error) => {
                    log::debug!("{NAME}: lookup {uri} parse failed: {error}");
                    continue;
                }
            };
            if let Some(exact) = results
                .data
                .iter()
                .find(|item| item.name.eq_ignore_ascii_case(query.trim()))
            {
                return Some((uri, exact.id));
            }
        }
        None
    }

    /// Ids of the configured language, resolved from `/api/languages`.
    fn resolved_lang_ids(&self) -> Vec<i64> {
        if self.site.lang == "all" {
            return Vec::new();
        }
        let Some(slug) = lang_slug(&self.site.lang) else {
            log::warn!(
                "{NAME}: language {} is not published by the live site",
                self.site.lang
            );
            return Vec::new();
        };
        let mut guard = match self.languages.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            *guard = Some(self.fetch_languages());
        }
        let table = guard.as_ref().map(|table| table.as_slice()).unwrap_or(&[]);
        let ids: Vec<i64> = table
            .iter()
            .filter(|(language_slug, _)| language_slug == slug)
            .map(|(_, id)| *id)
            .collect();
        if ids.is_empty() {
            log::warn!(
                "{NAME}: language {} ({slug}) is not available on {}",
                self.site.lang,
                self.site.base()
            );
        }
        ids
    }

    fn fetch_languages(&self) -> Vec<(String, i64)> {
        let mut table: Vec<(String, i64)> = Vec::new();
        let mut page = 1;
        while page <= 10 {
            let path = format!("/api/languages?page={page}");
            let body = match self.api_get(&path) {
                Ok(body) => body,
                Err(error) => {
                    log::warn!("{NAME}: fetching languages failed: {error:#}");
                    break;
                }
            };
            let response: ResponseDto<Vec<LanguageDto>> = match serde_json::from_str(&body) {
                Ok(response) => response,
                Err(error) => {
                    log::warn!("{NAME}: parsing languages failed: {error}");
                    break;
                }
            };
            table.extend(
                response
                    .data
                    .into_iter()
                    .map(|language| (language.slug, language.id)),
            );
            if response.next_page_url.is_none() {
                break;
            }
            page += 1;
        }
        table
    }

    fn parse_listing(&self, body: &str) -> Result<Vec<MangaInfo>> {
        let response: ResponseDto<Vec<ComicDto>> = serde_json::from_str(body)
            .context("parsing HentaiHand comics response")?;
        Ok(response
            .data
            .into_iter()
            .map(|comic| MangaInfo {
                source_id: self.site.id,
                title: comic.title,
                author: Vec::new(),
                genre: Vec::new(),
                status: None,
                description: None,
                path: path_for(&comic.slug),
                cover_url: comic
                    .image_url
                    .or(comic.thumb_url)
                    .unwrap_or_default(),
            })
            .collect())
    }

    fn detail(&self, path: &str) -> Result<ComicDetailDto> {
        let slug = slug_from_path(path);
        let body = self.api_get(&format!("/api/comics/{slug}"))?;
        serde_json::from_str(&body).with_context(|| format!("{NAME}: parsing details for {slug}"))
    }
}

#[derive(Debug, Deserialize)]
struct LookupDto {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct LanguageDto {
    id: i64,
    slug: String,
}

impl Extension for HentaiHand {
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
        let path = self.comic_url(page.max(1), "popular", None, true)?;
        self.parse_listing(&self.api_get(&path)?)
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        let path = self.comic_url(page.max(1), "latest", None, true)?;
        self.parse_listing(&self.api_get(&path)?)
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        let query = query.trim();
        let path = self.comic_url(page.max(1), "search", Some(query), true)?;
        let mut results = self.parse_listing(&self.api_get(&path)?)?;
        if results.is_empty() && !self.resolved_lang_ids().is_empty() {
            // A language-filtered search can legitimately be empty when the
            // site has no match in that language; fall back to a search across
            // every language so the source still returns real results.
            log::debug!("{NAME}: no match for {query:?} in {}, retrying unfiltered", self.site.lang);
            let fallback = self.comic_url(page.max(1), "search", Some(query), false)?;
            results = self.parse_listing(&self.api_get(&fallback)?)?;
        }
        Ok(results)
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let slug = slug_from_path(&path);
        let comic = self.detail(&path)?;
        let status = match comic.status.as_deref() {
            Some("ongoing") | Some("onhold") => Some("Ongoing".to_string()),
            Some("complete") | Some("canceled") => Some("Completed".to_string()),
            _ => Some("Completed".to_string()),
        };
        let author = names(&comic.authors)
            .or_else(|| names(&comic.artists))
            .map(|value| value.split(", ").map(str::to_string).collect())
            .unwrap_or_default();
        let mut genre: Vec<String> = Vec::new();
        for group in [&comic.tags, &comic.relationships] {
            if let Some(value) = names(group) {
                genre.extend(value.split(", ").map(str::to_string));
            }
        }
        let mut description_parts: Vec<String> = Vec::new();
        let mut push = |label: &str, value: Option<String>| {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                description_parts.push(format!("{label}: {value}"));
            }
        };
        push("Alternative Title", comic.alternative_title.clone());
        push("Groups", names(&comic.groups));
        push("Description", comic.description.clone());
        push("Pages", comic.pages.map(|pages| pages.to_string()));
        push("Category", comic.category.as_ref().map(|value| value.name.clone()));
        push("Language", comic.language.as_ref().map(|value| value.name.clone()));
        push("Parodies", names(&comic.parodies));
        push("Characters", names(&comic.characters));

        Ok(MangaInfo {
            source_id: self.site.id,
            title: comic.title,
            author,
            genre,
            status,
            description: if description_parts.is_empty() {
                None
            } else {
                Some(description_parts.join("\n\n"))
            },
            path: path_for(&slug),
            cover_url: comic.image_url.or(comic.thumb_url).unwrap_or_default(),
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        // The theme's `chapters = false`: details and the single chapter come
        // from the same endpoint (`/api/comics/<slug>`).
        let comic = self.detail(&path)?;
        let slug = comic.slug.unwrap_or_else(|| slug_from_path(&path));
        Ok(vec![ChapterInfo {
            source_id: self.site.id,
            title: "Chapter".to_string(),
            path: slug,
            number: 1.0,
            scanlator: None,
            uploaded: 0,
        }])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let slug = slug_from_path(&path);
        let body = self.api_get(&format!("/api/comics/{slug}/images"))?;
        let response: ImagesDto =
            serde_json::from_str(&body).with_context(|| format!("{NAME}: parsing pages for {slug}"))?;
        let mut images = response.images;
        images.sort_by_key(|image| image.page);
        let pages: Vec<String> = images
            .into_iter()
            .map(|image| image.source_url)
            .filter(|url| !url.is_empty())
            .collect();
        if pages.is_empty() {
            return Err(anyhow!("{NAME}: no pages found for {slug}"));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let referer = self.site.referer();
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=2u32 {
            let result = if attempt == 1 {
                self.client
                    .fetch_image_with_headers(&url, Some(&referer), &[])
            } else {
                self.client.fetch_image_with_headers(&url, None, &[])
            };
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    log::debug!("{NAME}: image attempt {attempt} failed for {url}: {error:#}");
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("{NAME}: no image bytes returned")))
            .with_context(|| format!("downloading {url}"))
    }
}
