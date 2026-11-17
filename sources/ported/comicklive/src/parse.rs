//! Server-rendered HTML helpers.
//!
//! Comick boots its frontend with JSON blobs embedded in
//! `<script type="application/json">` elements: `#comic-data` on comic detail
//! pages and `#sv-data` on chapter reader/search pages. The surrounding HTML
//! is a thicket of Alpine.js markup, so rather than building a DOM (Comick's
//! markup is malformed enough that html5ever warns while parsing it) scan for
//! the script tag and return its raw text for serde to decode.

/// Return the raw text content of the `<script type="application/json">`
/// element with the given `id`, if present.
pub fn json_element(html: &str, id: &str) -> Option<String> {
    let double_quoted = format!("id=\"{id}\"");
    let single_quoted = format!("id='{id}'");
    let id_at = html
        .find(&double_quoted)
        .or_else(|| html.find(&single_quoted))?;

    // The id must belong to the opening tag of the script, i.e. the last `<`
    // before it starts a `script` tag that also declares JSON.
    let tag_start = html[..id_at].rfind('<')?;
    let tag_end = id_at + html[id_at..].find('>')?;
    let tag = &html[tag_start..tag_end];
    if !tag_name(tag).eq_ignore_ascii_case("script") || !tag.contains("application/json") {
        return None;
    }

    let content_start = tag_end + 1;
    let content_end = html[content_start..].find("</script")? + content_start;
    Some(html[content_start..content_end].to_string())
}

/// Name of the element in an opening tag such as `<script type="...">`.
fn tag_name(tag: &str) -> &str {
    tag.trim_start_matches('<')
        .split(|character: char| character.is_whitespace() || character == '>' || character == '/')
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn extracts_json_from_script_element() {
        let html = r#"<html><head>
            <script type="application/json" id="comic-data">
                {"title": "A & B", "md_titles": []}
            </script>
            <script type="application/json" id="sv-data">{"chapter":{}}</script>
        </head><body></body></html>"#;
        assert_eq!(
            json_element(html, "comic-data").as_deref(),
            Some("\n                {\"title\": \"A & B\", \"md_titles\": []}\n            ")
        );
        assert_eq!(
            json_element(html, "sv-data").as_deref(),
            Some("{\"chapter\":{}}")
        );
        assert_eq!(json_element(html, "missing"), None);
    }

    #[test]
    fn ignores_non_json_and_non_script_elements() {
        let html = r#"<html><body>
            <script id="comic-data">window.x = 1;</script>
            <div id="sv-data">{"chapter":{}}</div>
            <script type="application/json" data-id="x" id="real">{"ok":true}</script>
        </body></html>"#;
        assert_eq!(json_element(html, "comic-data"), None);
        assert_eq!(json_element(html, "sv-data"), None);
        assert_eq!(json_element(html, "real").as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn handles_single_quoted_ids() {
        let html = r#"<script type='application/json' id='sv-data'>{"a":1}</script>"#;
        assert_eq!(json_element(html, "sv-data").as_deref(), Some("{\"a\":1}"));
    }
}
