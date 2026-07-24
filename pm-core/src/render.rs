//! Template rendering: {{placeholder}} substitution.

use std::collections::BTreeMap;

/// Returns placeholder names in order of first appearance (deduplicated).
pub fn placeholders(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // escaped \{{ -> not a placeholder
            if i > 0 && bytes[i - 1] == b'\\' {
                i += 2;
                continue;
            }
            if let Some(end) = content[i + 2..].find("}}") {
                let name = &content[i + 2..i + 2 + end];
                if is_valid_name(name) {
                    if !out.iter().any(|n| n == name) {
                        out.push(name.to_string());
                    }
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub struct RenderResult {
    pub text: String,
    /// placeholders that had no (non-empty) value and were left untouched
    pub missing: Vec<String>,
}

/// Renders content with the given values. A placeholder is substituted only
/// when a NON-EMPTY value exists; an unknown name or an empty value leaves
/// `{{name}}` in place and is reported in `missing`. `\{{` unescapes to `{{`.
///
/// Empty counts as missing because every recipe carries every variable, so
/// "not filled in" is expressed as an empty value rather than a missing key.
pub fn render(content: &str, values: &BTreeMap<&str, &str>) -> RenderResult {
    let mut text = String::with_capacity(content.len());
    let mut missing: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 2 < bytes.len() && bytes[i + 1] == b'{' && bytes[i + 2] == b'{'
        {
            text.push_str("{{");
            i += 3;
            continue;
        }
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = content[i + 2..].find("}}") {
                let name = &content[i + 2..i + 2 + end];
                if is_valid_name(name) {
                    match values.get(name) {
                        Some(v) if !v.is_empty() => text.push_str(v),
                        _ => {
                            text.push_str("{{");
                            text.push_str(name);
                            text.push_str("}}");
                            if !missing.iter().any(|m| m == name) {
                                missing.push(name.to_string());
                            }
                        }
                    }
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        // advance one UTF-8 character
        let ch_len = utf8_char_len(bytes[i]);
        text.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    RenderResult { text, missing }
}

/// Rewrites every `{{old}}` placeholder to `{{new}}`, leaving escaped
/// `\{{old}}` and any other text untouched.
///
/// A plain `content.replace("{{old}}", "{{new}}")` would also rewrite the
/// escaped form, so the same scanner as `render` is used here.
pub fn rename_placeholder(content: &str, old: &str, new: &str) -> String {
    if old == new || !is_valid_name(old) || !is_valid_name(new) {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // keep the escape sequence verbatim
        if bytes[i] == b'\\' && i + 2 < bytes.len() && bytes[i + 1] == b'{' && bytes[i + 2] == b'{'
        {
            out.push_str("\\{{");
            i += 3;
            continue;
        }
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = content[i + 2..].find("}}") {
                let name = &content[i + 2..i + 2 + end];
                if is_valid_name(name) {
                    out.push_str("{{");
                    out.push_str(if name == old { new } else { name });
                    out.push_str("}}");
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals<'a>(pairs: &[(&'a str, &'a str)]) -> BTreeMap<&'a str, &'a str> {
        pairs.iter().cloned().collect()
    }

    #[test]
    fn extracts_placeholders() {
        let c = "A {{jedna}} b {{dva}} c {{jedna}} {{Neplatná}} {{bad name}}";
        assert_eq!(placeholders(c), vec!["jedna", "dva"]);
    }

    #[test]
    fn renders_full() {
        let r = render(
            "Cesta: {{cesta}}, jazyk: {{jazyk}}.",
            &vals(&[("cesta", "C:\\X"), ("jazyk", "rust")]),
        );
        assert_eq!(r.text, "Cesta: C:\\X, jazyk: rust.");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn reports_missing() {
        let r = render("{{a}} {{b}} {{a}}", &vals(&[("a", "1")]));
        assert_eq!(r.text, "1 {{b}} 1");
        assert_eq!(r.missing, vec!["b"]);
    }

    #[test]
    fn escape_keeps_literal() {
        let r = render("ukázka \\{{ne_promenna}} a {{x}}", &vals(&[("x", "OK")]));
        assert_eq!(r.text, "ukázka {{ne_promenna}} a OK");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn czech_text_untouched() {
        let r = render("Žluťoučký {{kun}} pěl ďábelské ódy", &vals(&[("kun", "kůň")]));
        assert_eq!(r.text, "Žluťoučký kůň pěl ďábelské ódy");
    }

    #[test]
    fn unclosed_left_alone() {
        let r = render("{{neuzavreno a dál", &vals(&[]));
        assert_eq!(r.text, "{{neuzavreno a dál");
    }

    #[test]
    fn empty_value_is_missing() {
        // every recipe carries every variable, so "not filled in" is an
        // empty value - it must behave exactly like an absent key
        let r = render("{{a}}/{{b}}", &vals(&[("a", ""), ("b", "B")]));
        assert_eq!(r.text, "{{a}}/B");
        assert_eq!(r.missing, vec!["a"]);
    }

    #[test]
    fn renames_placeholder() {
        let out = rename_placeholder("{{a}} x {{b}} {{a}}", "a", "c");
        assert_eq!(out, "{{c}} x {{b}} {{c}}");
    }

    #[test]
    fn rename_respects_escaped_placeholder() {
        let out = rename_placeholder("\\{{a}} a {{a}}", "a", "c");
        assert_eq!(out, "\\{{a}} a {{c}}", "escaped form must stay untouched");
    }

    #[test]
    fn rename_ignores_invalid_and_noop() {
        assert_eq!(rename_placeholder("{{a}}", "a", "a"), "{{a}}");
        assert_eq!(rename_placeholder("{{a}}", "a", "Špatný"), "{{a}}");
        assert_eq!(rename_placeholder("{{ne platny}}", "a", "c"), "{{ne platny}}");
    }
}
