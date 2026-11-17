//! JSON shapes served by Comick's public API and embedded in its SSR pages.
//!
//! Everything here was verified against the live site:
//!
//! * `/api/comics/top`, `/api/search`, `/api/chapters/latest` return
//!   `{ data: [...], next_cursor?: "...", pagination?: {...} }`.
//! * `/api/comics/{slug}/chapter-list` adds a `pagination` envelope with
//!   `last_page` (60 chapters per page at the time of writing).
//! * the comic detail page embeds the `#comic-data` blob, the chapter reader
//!   page embeds `#sv-data`.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

/// Top-level `data` wrapper used by `/api/comics/top`.
#[derive(Debug, Deserialize)]
pub struct DataResponse<T> {
    #[serde(default)]
    pub data: T,
}

/// A comic as it appears in a listing endpoint.
#[derive(Debug, Deserialize)]
pub struct BrowseComic {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub default_thumbnail: Option<String>,
}

/// Cursor-paginated listing (`/api/search`, `/api/chapters/latest`).
#[derive(Debug, Default, Deserialize)]
pub struct CursorResponse {
    #[serde(default)]
    pub data: Vec<BrowseComic>,
    /// Opaque Laravel cursor; absent on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// `/api/comics/{slug}/chapter-list` response.
#[derive(Debug, Default, Deserialize)]
pub struct ChapterListResponse {
    #[serde(default)]
    pub data: Vec<ChapterDto>,
    #[serde(default)]
    pub pagination: Pagination,
}

#[derive(Debug, Default, Deserialize)]
pub struct Pagination {
    #[serde(default)]
    pub last_page: i64,
}

/// One chapter row from the chapter-list endpoint.
#[derive(Debug, Deserialize)]
pub struct ChapterDto {
    /// Opaque chapter id used in the reader URL.
    pub hid: String,
    /// Chapter number as a string ("12", "12.5", ...); may be empty.
    #[serde(default, deserialize_with = "opt_flexible_string")]
    pub chap: Option<String>,
    #[serde(default, deserialize_with = "opt_flexible_string")]
    pub vol: Option<String>,
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub group_name: Option<Vec<String>>,
}

fn default_lang() -> String {
    "en".to_string()
}

fn opt_flexible_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => Some(text),
        Some(serde_json::Value::Number(number)) => Some(number.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

/// Comic object embedded in the detail page under `#comic-data`.
#[derive(Debug, Deserialize)]
pub struct ComicData {
    pub title: String,
    pub slug: String,
    #[serde(default)]
    pub default_thumbnail: Option<String>,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(default)]
    pub translation_completed: bool,
    #[serde(default)]
    pub authors: Vec<Name>,
    #[serde(default)]
    pub artists: Vec<Name>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub content_rating: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default, rename = "md_comic_md_genres")]
    pub genres: Vec<GenreWrapper>,
    /// `md_titles` is normally an array but some payloads key it by id, which
    /// the Kotlin original handles with a transforming serializer.
    #[serde(default, rename = "md_titles")]
    pub titles: Option<Titles>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Name {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct GenreWrapper {
    #[serde(default)]
    pub md_genres: Name,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Titles {
    List(Vec<Title>),
    Map(HashMap<String, Title>),
}

#[derive(Debug, Deserialize)]
pub struct Title {
    #[serde(default)]
    pub title: String,
}

/// Reader page payload embedded under `#sv-data`.
#[derive(Debug, Deserialize)]
pub struct PageListData {
    pub chapter: ChapterData,
}

#[derive(Debug, Deserialize)]
pub struct ChapterData {
    #[serde(default)]
    pub images: Vec<PageImage>,
}

#[derive(Debug, Deserialize)]
pub struct PageImage {
    #[serde(default)]
    pub url: String,
}
