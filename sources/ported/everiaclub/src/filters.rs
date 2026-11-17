//! Search filters, ported from `EveriaClub.getFilterList`.
//!
//! The original fetches the category and tag IDs at runtime. Tanoshi's
//! `filter_list()` cannot perform network requests, so the category ids are the
//! site's stable defaults (verified live against the REST categories endpoint)
//! and the dynamic tri-state tag group is omitted.

use tanoshi_lib::prelude::{Input, InputType};

/// `(label, category id)` pairs: the original's defaults.
const CATEGORIES: [(&str, &str); 6] = [
    ("Any", ""),
    ("China", "42"),
    ("Cosplay", "7"),
    ("Japan", "2"),
    ("Korea", "11"),
    ("Thailand", "1984"),
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

/// Category id selected by the client, if any (empty means "Any").
pub(crate) fn category(filters: &[Input]) -> Option<String> {
    filters.iter().find_map(|filter| match filter {
        Input::Select {
            name,
            state: Some(state),
            ..
        } if name == "Category" => {
            let index = usize::try_from(*state).ok()?;
            CATEGORIES
                .get(index)
                .map(|(_, id)| (*id).to_string())
                .filter(|id| !id.is_empty())
        }
        _ => None,
    })
}
