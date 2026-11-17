//! Search filters, ported from `CosplayTele.getFilterList`.
//!
//! Only one filter is applied at a time, mirroring the original's
//! "Only one filter will be applied!" note.

use tanoshi_lib::prelude::{Input, InputType};

/// `(label, archive path)` pairs, in the original filter order.
///
/// The original's `category/nude` and `category/no-nude` slugs now 404 on the
/// live site; the current archive slugs are used instead (verified against a
/// post's category links: `cosplay-nudee`, `cosplay-ero`, `cosplay`).
const CATEGORIES: [(&str, &str); 4] = [
    ("All", ""),
    ("Cosplay Nude", "category/cosplay-nudee"),
    ("Cosplay Ero", "category/cosplay-ero"),
    ("Cosplay", "category/cosplay"),
];

pub(crate) fn filter_list() -> Vec<Input> {
    vec![Input::Select {
        name: "Category".to_string(),
        values: CATEGORIES
            .iter()
            .map(|(label, _)| InputType::String((*label).to_string()))
            .collect(),
        state: Some(0),
    }]
}

/// Archive path selected by the client, if any (empty means "All").
pub(crate) fn category(filters: &[Input]) -> Option<String> {
    filters.iter().find_map(|filter| match filter {
        Input::Select {
            name,
            state: Some(state),
            ..
        } if name == "Category" => {
            let index = usize::try_from(*state).ok()?;
            CATEGORIES.get(index).map(|(_, path)| (*path).to_string())
        }
        _ => None,
    })
}
