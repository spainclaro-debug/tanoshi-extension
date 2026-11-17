//! Template for a new Tanoshi extension.
//!
//! Copy this directory to `sources/ported/<your-source>/`, rename the package in
//! `Cargo.toml`, then replace the placeholder values and implement the
//! [`Extension`] trait methods below. Keep the crate a `cdylib` and keep the
//! `export_extension!` macro call — it is what makes the plugin loadable by the
//! Tanoshi server.
//!
//! Build it for Android with:
//!
//! ```bash
//! bash android/native/build_extension.sh --crate android/extensions/sources/ported/<your-source>
//! ```
//!
//! See the ported extensions in this directory for complete examples
//! (`qiscans`, `atsumaru`, …) and the `networking` crate
//! (`luigi311/tanoshi-extensions/common/networking`) for rate-limited HTTP
//! helpers, JSON helpers and referer/cookie support.

use anyhow::{Result, bail};
use tanoshi_lib::prelude::{ChapterInfo, Extension, Input, Lang, MangaInfo, SourceInfo};

// Registers the exported `plugin_declaration` symbol used by the server.
extension_utils::export_extension!(register, MyExtension, NAME);

/// Unique source id. Check `tanoshi-extensions/docs/sources.md` for reserved ids
/// and pick an unused value before publishing.
const ID: i64 = 0;
const NAME: &str = "MyExtension";
const URL: &str = "https://example.com";
const ICON_URL: &str = "https://example.com/favicon.ico";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The plugin entry point. Keep it cheap to construct; create HTTP clients here
/// (see `networking::build_rate_limited_ureq_agent`) rather than per request.
pub struct MyExtension;

impl Default for MyExtension {
    fn default() -> Self {
        Self
    }
}

impl Extension for MyExtension {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: ID,
            name: NAME.to_string(),
            url: URL.to_string(),
            version: VERSION,
            icon: ICON_URL,
            languages: Lang::Single("en".to_string()),
            nsfw: false,
        }
    }

    fn get_popular_manga(&self, _page: i64) -> Result<Vec<MangaInfo>> {
        // TODO: fetch the popular listing and map it to MangaInfo.
        bail!("{NAME}: get_popular_manga is not implemented yet")
    }

    fn get_latest_manga(&self, _page: i64) -> Result<Vec<MangaInfo>> {
        // TODO: fetch the latest listing and map it to MangaInfo.
        bail!("{NAME}: get_latest_manga is not implemented yet")
    }

    fn search_manga(
        &self,
        _page: i64,
        _query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        // TODO: search the source. You can ignore filters for a first port.
        bail!("{NAME}: search_manga is not implemented yet")
    }

    fn get_manga_detail(&self, _path: String) -> Result<MangaInfo> {
        // TODO: `path` is the `path` field from MangaInfo (usually a slug/id).
        bail!("{NAME}: get_manga_detail is not implemented yet")
    }

    fn get_chapters(&self, _path: String) -> Result<Vec<ChapterInfo>> {
        // TODO: return all chapters for the manga at `path`.
        bail!("{NAME}: get_chapters is not implemented yet")
    }

    fn get_pages(&self, _path: String) -> Result<Vec<String>> {
        // TODO: return the page/image URLs for a chapter `path`.
        bail!("{NAME}: get_pages is not implemented yet")
    }

    fn get_image_bytes(&self, _url: String) -> Result<extension_utils::bytes::Bytes> {
        // TODO: fetch the image. Use `networking::RateLimitedAgent` and send the
        // required Referer for the source.
        bail!("{NAME}: get_image_bytes is not implemented yet")
    }
}
