//! **Kemono multisrc** — one Rust plugin covering the Keiyoushi `kemono`
//! multisrc theme (Coomer and Kemono).
//!
//! A Tanoshi plugin registers a single source, so this binary is published once
//! per site in the extension catalog: every catalog entry points at the same
//! `.so` and the installed file name selects the site configuration embedded in
//! [`sites.json`](../sites.json). The server starts an extension worker with
//! `--plugin <path>`; `std::env::args()` is empty inside a `dlopen`ed library on
//! Android/Termux, so the plugin reads `/proc/self/cmdline`, falls back to
//! `dladdr` on its own code and finally to `TANOSHI_MULTISRC_SITE` (tests).
//!
//! Theme behaviour (mirrored from `lib-multisrc/kemono/Kemono.kt`, verified
//! against the live APIs):
//! - creators come from the single `GET /api/v1/creators` JSON index and are
//!   sorted client-side: popular = `favorited` desc, latest = `updated` desc,
//!   search = case-insensitive name filter; 50 creators per page;
//! - every `/api/v1` request must send `Accept: text/css` — the ddos-guard
//!   front end answers HTTP 403 otherwise (verified with curl);
//! - detail fetches the creator profile (`/api/v1/{service}/user/{id}/profile`);
//! - chapters are the creator's posts (`/api/v1/{service}/user/{id}/posts?o=N`,
//!   50 per page, first two pages), keeping only posts that contain images;
//! - pages come from the post detail (`/api/v1/{service}/user/{id}/post/<post>`)
//!   mapped to `thumbnail/data/<path>` URLs, because the full-size `nX.<site>`
//!   file servers are not reachable (requests stall); the thumbnail endpoint
//!   returns real images up to 800px;
//! - images are fetched with a site `Referer` (and retried).
//!
//! Both sites were verified against the live API.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use networking::{RateLimitedAgent, build_rate_limited_ureq_agent};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

mod dto;
mod parse;

use dto::{Creator, PostWrapped, Profile};

extension_utils::export_extension!(register, Kemono, NAME);

const NAME: &str = "Kemono";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";
/// Creators per page (the theme's `PAGE_CREATORS_LIMIT`).
const PAGE_SIZE: i64 = 50;
/// Posts per API response (the theme's `PAGE_POST_LIMIT`).
const POST_PAGE_SIZE: usize = 50;
/// Post pages merged into the chapter list (the theme defaults to one page).
const POST_PAGES: usize = 2;
/// The creators index is a single large response; cache it like the theme's
/// 30-minute OkHttp max-stale cache, but shorter to pick up new creators.
const CREATORS_TTL: Duration = Duration::from_secs(600);

/// One site/source configuration from the embedded `sites.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    /// Catalog/plugin key, e.g. `kemono-coomer`.
    pub key: String,
    /// Display name shown by the server, e.g. `Coomer`.
    pub name: String,
    /// Site root, no trailing slash, e.g. `https://coomer.st`.
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

pub struct Kemono {
    site: Site,
    client: RateLimitedAgent,
    /// Cached `/api/v1/creators` index: fetched once, reused for a short TTL.
    creators: Mutex<Option<(Instant, Arc<Vec<Creator>>)>>,
}

impl Default for Kemono {
    fn default() -> Self {
        let site = selected_site();
        log::info!("{NAME}: selected site {} ({})", site.key, site.url);
        Self {
            site,
            client: build_rate_limited_ureq_agent(Some(USER_AGENT), Some(1.0)),
            creators: Mutex::new(None),
        }
    }
}

impl Kemono {
    /// GET a JSON document with the theme's mandatory `Accept: text/css` quirk.
    fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        log::debug!("{NAME}: GET {url}");
        let referer = self.site.referer();
        let body = self
            .client
            .fetch_text_with_headers(
                url,
                &[("Accept", "text/css"), ("Referer", referer.as_str())],
            )
            .with_context(|| format!("{}: requesting {url}", self.site.key))?;
        serde_json::from_str(&body)
            .with_context(|| format!("{}: decoding JSON from {url}", self.site.key))
    }

    fn fetch_creators(&self) -> Result<Arc<Vec<Creator>>> {
        if let Ok(guard) = self.creators.lock() {
            if let Some((fetched_at, cached)) = guard.as_ref() {
                if fetched_at.elapsed() < CREATORS_TTL {
                    return Ok(cached.clone());
                }
            }
        }

        let url = format!("{}/creators", parse::api_url(&self.site));
        let fetched: Result<Vec<Creator>> = self.get_json(&url);
        match fetched {
            Ok(mut list) => {
                // Discord "creators" are servers with no post galleries.
                list.retain(|creator| !creator.service.eq_ignore_ascii_case("discord"));
                let list = Arc::new(list);
                if let Ok(mut guard) = self.creators.lock() {
                    *guard = Some((Instant::now(), list.clone()));
                }
                Ok(list)
            }
            Err(error) => {
                // Serve a stale cache rather than failing the whole browse page.
                if let Ok(guard) = self.creators.lock() {
                    if let Some((_, cached)) = guard.as_ref() {
                        log::warn!("{NAME}: creators refresh failed, using cached list: {error:#}");
                        return Ok(cached.clone());
                    }
                }
                Err(error)
            }
        }
    }

    fn creator_listing(&self, page: i64, mode: &str, query: &str) -> Result<Vec<MangaInfo>> {
        let creators = self.fetch_creators()?;
        let query_lower = query.to_lowercase();
        let mut matched: Vec<&Creator> = creators
            .iter()
            .filter(|creator| {
                mode != "search" || creator.name.to_lowercase().contains(&query_lower)
            })
            .collect();

        match mode {
            "latest" => matched.sort_by(|a, b| {
                let a = a.updated.as_ref().map(|value| value.epoch_seconds()).unwrap_or(0);
                let b = b.updated.as_ref().map(|value| value.epoch_seconds()).unwrap_or(0);
                b.cmp(&a)
            }),
            // Popular and search default to the popularity sort of the theme.
            _ => matched.sort_by(|a, b| b.favorited.cmp(&a.favorited)),
        }

        let page = page.max(1);
        let from = ((page - 1) * PAGE_SIZE) as usize;
        Ok(matched
            .into_iter()
            .skip(from)
            .take(PAGE_SIZE as usize)
            .map(|creator| creator_to_manga(&self.site, creator))
            .collect())
    }

    fn profile(&self, service: &str, user: &str) -> Result<Profile> {
        let url = format!(
            "{}/{service}/user/{user}/profile",
            parse::api_url(&self.site)
        );
        self.get_json(&url)
    }

    /// Posts whose paths belong to a creator, newest first.
    fn creator_posts(&self, service: &str, user: &str) -> Result<Vec<dto::Post>> {
        let mut out: Vec<dto::Post> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for page in 0..POST_PAGES {
            let offset = page * POST_PAGE_SIZE;
            let url = format!(
                "{}/{service}/user/{user}/posts?o={offset}",
                parse::api_url(&self.site)
            );
            log::debug!("{NAME}: GET {url}");
            let body = self
                .client
                .fetch_text_with_headers(
                    &url,
                    &[("Accept", "text/css"), ("Referer", self.site.referer().as_str())],
                )
                .with_context(|| format!("{}: requesting {url}", self.site.key))?;
            let posts: Vec<dto::Post> = serde_json::from_str(&body)
                .with_context(|| format!("{}: decoding posts from {url}", self.site.key))?;
            let received = posts.len();
            for post in posts {
                if seen.contains(&post.id) {
                    continue;
                }
                seen.push(post.id.clone());
                out.push(post);
            }
            if received < POST_PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }
}

fn creator_to_manga(site: &Site, creator: &Creator) -> MangaInfo {
    let title = if creator.name.trim().is_empty() {
        creator.id.clone()
    } else {
        creator.name.trim().to_string()
    };
    MangaInfo {
        source_id: site.id,
        title,
        author: vec![parse::service_name(&creator.service)],
        genre: Vec::new(),
        status: None,
        description: None,
        path: parse::creator_path(&creator.service, &creator.id),
        cover_url: parse::creator_icon(site, &creator.service, &creator.id),
    }
}

impl Extension for Kemono {
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
        self.creator_listing(page, "popular", "")
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.creator_listing(page, "latest", "")
    }

    fn search_manga(
        &self,
        page: i64,
        query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        let query = query.unwrap_or_default();
        self.creator_listing(page, "search", query.trim())
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        let (service, user) = parse::creator_parts(&path)?;
        let profile = self.profile(&service, &user).with_context(|| {
            format!("{}: fetching creator profile for {path}", self.site.key)
        })?;
        let service_display = parse::service_name(if profile.service.is_empty() {
            &service
        } else {
            &profile.service
        });
        let title = if profile.name.trim().is_empty() {
            user.clone()
        } else {
            profile.name.trim().to_string()
        };
        let description = match profile.post_count {
            Some(count) if count > 0 => Some(format!(
                "{service_display} creator with {count} indexed posts"
            )),
            _ => Some(format!("{service_display} creator")),
        };
        Ok(MangaInfo {
            source_id: self.site.id,
            title,
            author: vec![service_display],
            genre: Vec::new(),
            status: None,
            description,
            path: parse::creator_path(&service, &user),
            cover_url: parse::creator_icon(&self.site, &service, &user),
        })
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        let (service, user) = parse::creator_parts(&path)?;
        let posts = self.creator_posts(&service, &user)?;
        let mut chapters: Vec<ChapterInfo> = Vec::new();
        for post in &posts {
            if post.image_paths().is_empty() {
                continue;
            }
            let service = if post.service.is_empty() {
                service.as_str()
            } else {
                post.service.as_str()
            };
            let user = if post.user.is_empty() {
                user.as_str()
            } else {
                post.user.as_str()
            };
            chapters.push(ChapterInfo {
                source_id: self.site.id,
                title: post.display_title(),
                path: parse::post_path(service, user, &post.id),
                // The theme reports unknown numbering for every post.
                number: -2.0,
                scanlator: None,
                uploaded: post.epoch_millis(),
            });
        }
        Ok(chapters)
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        let url = parse::api_path(&self.site, &path);
        let wrapped: PostWrapped = self
            .get_json(&url)
            .with_context(|| format!("{}: fetching post {path}", self.site.key))?;
        let pages: Vec<String> = wrapped
            .post
            .image_paths()
            .iter()
            .map(|path| parse::thumbnail_url(&self.site, path))
            .collect();
        if pages.is_empty() {
            return Err(anyhow!(
                "{}: post {path} has no image attachments",
                self.site.key
            ));
        }
        Ok(pages)
    }

    fn get_image_bytes(&self, url: String) -> Result<extension_utils::bytes::Bytes> {
        let referer = self.site.referer();
        let mut last_error: Option<anyhow::Error> = None;
        for candidate in parse::image_candidates(&self.site, &url) {
            for attempt in 1..=2u32 {
                let fetch = if attempt == 1 {
                    self.client
                        .fetch_image_with_headers(&candidate, Some(&referer), &[])
                } else {
                    self.client.fetch_image_with_headers(&candidate, None, &[])
                };
                match fetch {
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => {
                        log::warn!(
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
