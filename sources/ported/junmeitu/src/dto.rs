//! JSON shape of the Junmeitu gallery AJAX response.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Dto {
    /// HTML `<img …>` snippet of the page image.
    #[serde(default)]
    pub pic: String,
}
