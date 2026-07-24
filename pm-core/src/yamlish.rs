//! Minimal YAML-subset reader/writer for the formats this app owns:
//! recipe files and prompt frontmatter. Both are written by this module in a
//! canonical form; hand-edited files must follow the same subset:
//!
//! - `key: value` at top level (value bare or double-quoted with JSON-style escapes)
//! - `key:` followed by an indented block of `  subkey: value` (one level, used for `values:`)
//! - inline string arrays: `key: ["a", "b"]` (JSON syntax)
//! - `#` comments and blank lines are ignored (only at line starts)

use crate::json::{write_json_string, Json};


#[derive(Debug, Clone, PartialEq, Default)]
pub struct YamlDoc {
    /// top-level scalar entries in original order
    pub scalars: Vec<(String, String)>,
    /// top-level string-array entries
    pub arrays: Vec<(String, Vec<String>)>,
    /// top-level nested map entries (one level deep)
    pub maps: Vec<(String, Vec<(String, String)>)>,
}

impl YamlDoc {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.scalars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    pub fn get_arr(&self, key: &str) -> Option<&Vec<String>> {
        self.arrays.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_map(&self, key: &str) -> Option<&Vec<(String, String)>> {
        self.maps.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(e) = self.scalars.iter_mut().find(|(k, _)| k == key) {
            e.1 = value.to_string();
        } else {
            self.scalars.push((key.to_string(), value.to_string()));
        }
    }
    pub fn set_arr(&mut self, key: &str, value: Vec<String>) {
        if let Some(e) = self.arrays.iter_mut().find(|(k, _)| k == key) {
            e.1 = value;
        } else {
            self.arrays.push((key.to_string(), value));
        }
    }
    pub fn set_map(&mut self, key: &str, value: Vec<(String, String)>) {
        if let Some(e) = self.maps.iter_mut().find(|(k, _)| k == key) {
            e.1 = value;
        } else {
            self.maps.push((key.to_string(), value));
        }
    }

    pub fn parse(input: &str) -> Result<YamlDoc, String> {
        let mut doc = YamlDoc::default();
        let mut lines = input.lines().enumerate().peekable();
        while let Some((ln, raw)) = lines.next() {
            let line = raw.trim_end();
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if line.starts_with(' ') {
                return Err(format!("line {}: unexpected indentation", ln + 1));
            }
            let (key, rest) = split_key(line, ln)?;
            let rest = rest.trim();
            if rest.is_empty() {
                // nested block
                let mut entries = Vec::new();
                while let Some((_, next)) = lines.peek() {
                    if next.starts_with("  ") && !next.trim().is_empty() {
                        let (ln2, sub) = lines.next().unwrap();
                        let sub = sub.trim_end();
                        let trimmed = sub.trim_start();
                        if trimmed.starts_with('#') {
                            continue;
                        }
                        let (k2, r2) = split_key(trimmed, ln2)?;
                        entries.push((k2, parse_scalar(r2.trim(), ln2)?));
                    } else if next.trim().is_empty() {
                        lines.next();
                    } else {
                        break;
                    }
                }
                doc.maps.push((key, entries));
            } else if rest.starts_with('[') {
                let arr = Json::parse(rest)
                    .map_err(|e| format!("line {}: bad array: {}", ln + 1, e))?;
                let items = arr
                    .as_arr()
                    .ok_or_else(|| format!("line {}: expected array", ln + 1))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .ok_or_else(|| format!("line {}: array items must be strings", ln + 1))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                doc.arrays.push((key, items));
            } else {
                doc.scalars.push((key, parse_scalar(rest, ln)?));
            }
        }
        Ok(doc)
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.scalars {
            out.push_str(k);
            out.push_str(": ");
            write_scalar(&mut out, v);
            out.push('\n');
        }
        for (k, items) in &self.arrays {
            out.push_str(k);
            out.push_str(": [");
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json_string(&mut out, it);
            }
            out.push_str("]\n");
        }
        for (k, entries) in &self.maps {
            out.push_str(k);
            out.push_str(":\n");
            for (k2, v2) in entries {
                out.push_str("  ");
                out.push_str(k2);
                out.push_str(": ");
                write_scalar(&mut out, v2);
                out.push('\n');
            }
        }
        out
    }
}

fn split_key(line: &str, ln: usize) -> Result<(String, &str), String> {
    let idx = line
        .find(':')
        .ok_or_else(|| format!("line {}: missing ':'", ln + 1))?;
    let key = line[..idx].trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("line {}: invalid key '{}'", ln + 1, key));
    }
    Ok((key.to_string(), &line[idx + 1..]))
}

fn parse_scalar(s: &str, ln: usize) -> Result<String, String> {
    if s.starts_with('"') {
        let v = Json::parse(s).map_err(|e| format!("line {}: bad string: {}", ln + 1, e))?;
        v.as_str()
            .map(|x| x.to_string())
            .ok_or_else(|| format!("line {}: expected string", ln + 1))
    } else {
        Ok(s.to_string())
    }
}

fn write_scalar(out: &mut String, v: &str) {
    // Quote whenever the bare form would be ambiguous or lossy.
    let needs_quotes = v.is_empty()
        || v.starts_with('"')
        || v.starts_with('[')
        || v.starts_with('#')
        || v.starts_with(' ')
        || v.ends_with(' ')
        || v.contains('\n')
        || v.contains('\r')
        || v.contains('\t')
        || v.contains(": ")
        || v.ends_with(':');
    if needs_quotes {
        write_json_string(out, v);
    } else {
        out.push_str(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_recipe_like() {
        let mut d = YamlDoc::default();
        d.set("id", "r-123");
        d.set("name", "Ukázkový projekt");
        d.set("updated", "2026-07-23T10:00:00Z");
        d.set_map(
            "values",
            vec![
                ("projekt_cesta".into(), "C:\\Projekty\\Ukazka".into()),
                ("poznamka".into(), "obsahuje: dvojtečku".into()),
                ("prazdna".into(), "".into()),
                ("viceradkova".into(), "a\nb".into()),
            ],
        );
        let text = d.dump();
        let parsed = YamlDoc::parse(&text).unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn roundtrip_frontmatter_like() {
        let mut d = YamlDoc::default();
        d.set("id", "p-1");
        d.set("title", "Code review – Rust");
        d.set_arr("recipes", vec!["r-1".into(), "r-2".into()]);
        d.set_arr("tags", vec![]);
        d.set("selected_recipe", "r-1");
        let parsed = YamlDoc::parse(&d.dump()).unwrap();
        assert_eq!(parsed, d);
        assert_eq!(parsed.get_arr("recipes").unwrap().len(), 2);
        assert!(parsed.get_arr("tags").unwrap().is_empty());
    }

    #[test]
    fn parses_hand_written() {
        let src = "# komentář\nid: abc\nname: Bez uvozovek\n\nvalues:\n  a: 1\n  # vnořený komentář\n  b: \"x\\ny\"\n";
        let d = YamlDoc::parse(src).unwrap();
        assert_eq!(d.get("name").unwrap(), "Bez uvozovek");
        let vals = d.get_map("values").unwrap();
        assert_eq!(vals[0], ("a".to_string(), "1".to_string()));
        assert_eq!(vals[1], ("b".to_string(), "x\ny".to_string()));
    }

    #[test]
    fn rejects_bad_keys() {
        assert!(YamlDoc::parse("bad key: 1").is_err());
        assert!(YamlDoc::parse("no-colon-line").is_err());
    }
}
