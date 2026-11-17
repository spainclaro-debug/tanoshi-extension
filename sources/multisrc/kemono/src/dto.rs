//! JSON DTOs for the Kemono theme's `api/v1` endpoints.
//!
//! Field types were verified against the live `coomer.st` and `kemono.cr` APIs:
//! the creators index returns `updated` as a Unix timestamp, while profiles use
//! ISO strings, so both forms are accepted.

use serde::Deserialize;

/// A creator from `GET /api/v1/creators` (the whole index in one response).
#[derive(Clone, Debug, Deserialize)]
pub struct Creator {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub service: String,
    /// `updated` is a Unix timestamp in the index but an ISO string in profiles.
    #[serde(default)]
    pub updated: Option<Timestamp>,
    #[serde(default)]
    pub favorited: i64,
}

/// Creator profile from `GET /api/v1/{service}/user/{id}/profile`.
#[derive(Clone, Debug, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub post_count: Option<i64>,
}

/// A post, which the theme exposes as one chapter.
#[derive(Clone, Debug, Deserialize)]
pub struct Post {
    pub id: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub added: Option<String>,
    #[serde(default)]
    pub edited: Option<String>,
    #[serde(default)]
    pub file: Option<FileEntry>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct FileEntry {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Attachment {
    #[serde(default)]
    pub path: String,
}

/// The post-detail endpoint wraps the post in an object.
#[derive(Clone, Debug, Deserialize)]
pub struct PostWrapped {
    pub post: Post,
}

/// Either a Unix timestamp (seconds) or an ISO-8601 local date-time string.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Timestamp {
    Seconds(i64),
    Text(String),
}

impl Timestamp {
    /// Unix seconds, or `0` when the value cannot be parsed.
    pub fn epoch_seconds(&self) -> i64 {
        match self {
            Self::Seconds(seconds) => *seconds,
            Self::Text(text) => crate::parse::parse_iso_seconds(text).unwrap_or(0),
        }
    }
}

/// Image attachments of a post, deduplicated, in display order (file first).
impl Post {
    pub fn image_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |path: &str| {
            let path = path.trim();
            if path.is_empty() {
                return;
            }
            if !is_image(path) {
                return;
            }
            if !out.iter().any(|existing| existing == path) {
                out.push(path.to_string());
            }
        };
        if let Some(file) = &self.file {
            if let Some(path) = file.path.as_deref() {
                push(path);
            }
        }
        for attachment in &self.attachments {
            push(&attachment.path);
        }
        out
    }

    /// Post date in Unix milliseconds (`edited` → `published` → `added`).
    pub fn epoch_millis(&self) -> i64 {
        for raw in [&self.edited, &self.published, &self.added]
            .into_iter()
            .flatten()
        {
            if let Some(seconds) = crate::parse::parse_iso_seconds(raw) {
                return seconds * 1000;
            }
        }
        0
    }

    /// Human readable title, falling back to the post date.
    pub fn display_title(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
        let millis = self.epoch_millis();
        if millis > 0 {
            format!("Post from {millis}")
        } else {
            format!("Post {}", self.id)
        }
    }
}

fn is_image(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((_, extension)) => matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "gif" | "jpeg" | "webp"
        ),
        None => false,
    }
}
