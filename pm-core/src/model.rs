//! Data model: Prompt, Recipe and (de)serialization to the on-disk formats.

use crate::json::Json;
use crate::yamlish::YamlDoc;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub id: String,
    pub title: String,
    pub created: String,
    pub updated: String,
    /// ids of assigned recipes (order = assignment order)
    pub recipes: Vec<String>,
    /// recipe id preselected in the sidebar; None = raw
    pub selected_recipe: Option<String>,
    pub tags: Vec<String>,
    /// template body with {{placeholders}}
    pub content: String,
    /// file name (without directory), filled by storage
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub updated: String,
    pub values: Vec<(String, String)>,
    pub file_name: String,
}

pub const FRONTMATTER_DELIM: &str = "---";

impl Prompt {
    pub fn to_markdown(&self) -> String {
        let mut fm = YamlDoc::default();
        fm.set("id", &self.id);
        fm.set("title", &self.title);
        fm.set("created", &self.created);
        fm.set("updated", &self.updated);
        fm.set_arr("recipes", self.recipes.clone());
        fm.set(
            "selected_recipe",
            self.selected_recipe.as_deref().unwrap_or(""),
        );
        fm.set_arr("tags", self.tags.clone());
        format!(
            "{}\n{}{}\n{}",
            FRONTMATTER_DELIM,
            fm.dump(),
            FRONTMATTER_DELIM,
            self.content
        )
    }

    pub fn from_markdown(text: &str, file_name: &str) -> Result<Prompt, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text); // BOM
        let normalized = text.replace("\r\n", "\n");
        let rest = normalized
            .strip_prefix(FRONTMATTER_DELIM)
            .ok_or("missing frontmatter open")?;
        let rest = rest.strip_prefix('\n').ok_or("missing newline after ---")?;
        let close = format!("\n{}\n", FRONTMATTER_DELIM);
        let idx = rest.find(&close).ok_or("missing frontmatter close")?;
        let fm_text = &rest[..idx + 1]; // keep trailing newline for the parser
        let content = &rest[idx + close.len()..];
        let fm = YamlDoc::parse(fm_text)?;
        let sel = fm.get("selected_recipe").unwrap_or("").to_string();
        Ok(Prompt {
            id: fm.get("id").ok_or("missing id")?.to_string(),
            title: fm.get("title").ok_or("missing title")?.to_string(),
            created: fm.get("created").unwrap_or("").to_string(),
            updated: fm.get("updated").unwrap_or("").to_string(),
            recipes: fm.get_arr("recipes").cloned().unwrap_or_default(),
            selected_recipe: if sel.is_empty() { None } else { Some(sel) },
            tags: fm.get_arr("tags").cloned().unwrap_or_default(),
            content: content.to_string(),
            file_name: file_name.to_string(),
        })
    }

    pub fn to_json(&self) -> Json {
        let mut o = BTreeMap::new();
        o.insert("id".into(), Json::str(&self.id));
        o.insert("title".into(), Json::str(&self.title));
        o.insert("created".into(), Json::str(&self.created));
        o.insert("updated".into(), Json::str(&self.updated));
        o.insert(
            "recipes".into(),
            Json::Arr(self.recipes.iter().map(|r| Json::str(r)).collect()),
        );
        o.insert(
            "selected_recipe".into(),
            match &self.selected_recipe {
                Some(s) => Json::str(s),
                None => Json::Null,
            },
        );
        o.insert(
            "tags".into(),
            Json::Arr(self.tags.iter().map(|t| Json::str(t)).collect()),
        );
        o.insert("content".into(), Json::str(&self.content));
        o.insert(
            "placeholders".into(),
            Json::Arr(
                crate::render::placeholders(&self.content)
                    .into_iter()
                    .map(|p| Json::Str(p))
                    .collect(),
            ),
        );
        Json::Obj(o)
    }
}

impl Recipe {
    pub fn to_yaml(&self) -> String {
        let mut d = YamlDoc::default();
        d.set("id", &self.id);
        d.set("name", &self.name);
        d.set("updated", &self.updated);
        d.set_map("values", self.values.clone());
        d.dump()
    }

    pub fn from_yaml(text: &str, file_name: &str) -> Result<Recipe, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let d = YamlDoc::parse(&text.replace("\r\n", "\n"))?;
        Ok(Recipe {
            id: d.get("id").ok_or("missing id")?.to_string(),
            name: d.get("name").ok_or("missing name")?.to_string(),
            updated: d.get("updated").unwrap_or("").to_string(),
            values: d.get_map("values").cloned().unwrap_or_default(),
            file_name: file_name.to_string(),
        })
    }

    pub fn to_json(&self) -> Json {
        let mut o = BTreeMap::new();
        o.insert("id".into(), Json::str(&self.id));
        o.insert("name".into(), Json::str(&self.name));
        o.insert("updated".into(), Json::str(&self.updated));
        let mut vals = BTreeMap::new();
        for (k, v) in &self.values {
            vals.insert(k.clone(), Json::str(v));
        }
        o.insert("values".into(), Json::Obj(vals));
        Json::Obj(o)
    }

    pub fn values_map(&self) -> BTreeMap<&str, &str> {
        self.values
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

/// Validates a variable name: [a-z0-9_]+ (lowercase by convention).
pub fn valid_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_prompt() -> Prompt {
        Prompt {
            id: "p-1".into(),
            title: "Code review – Rust".into(),
            created: "2026-07-23T10:00:00Z".into(),
            updated: "2026-07-23T11:00:00Z".into(),
            recipes: vec!["r-1".into(), "r-2".into()],
            selected_recipe: Some("r-1".into()),
            tags: vec![],
            content: "Zkontroluj {{projekt_cesta}}.\nJazyk: {{jazyk}}.".into(),
            file_name: "code-review.md".into(),
        }
    }

    #[test]
    fn prompt_roundtrip() {
        let p = sample_prompt();
        let md = p.to_markdown();
        let q = Prompt::from_markdown(&md, "code-review.md").unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn prompt_roundtrip_crlf_and_bom() {
        let p = sample_prompt();
        let md = format!("\u{feff}{}", p.to_markdown().replace('\n', "\r\n"));
        let q = Prompt::from_markdown(&md, "code-review.md").unwrap();
        assert_eq!(q.title, p.title);
        assert_eq!(q.content, p.content);
    }

    #[test]
    fn prompt_none_selected() {
        let mut p = sample_prompt();
        p.selected_recipe = None;
        let q = Prompt::from_markdown(&p.to_markdown(), "f.md").unwrap();
        assert_eq!(q.selected_recipe, None);
    }

    #[test]
    fn content_with_dashes_survives() {
        let mut p = sample_prompt();
        p.content = "text\n---\nještě text s --- uvnitř".into();
        let q = Prompt::from_markdown(&p.to_markdown(), "f.md").unwrap();
        assert_eq!(q.content, p.content);
    }

    #[test]
    fn recipe_roundtrip() {
        let r = Recipe {
            id: "r-1".into(),
            name: "Ukázkový projekt".into(),
            updated: "2026-07-23T10:00:00Z".into(),
            values: vec![
                ("projekt_cesta".into(), "C:\\Projekty\\Ukazka".into()),
                ("jazyk".into(), "rust".into()),
            ],
            file_name: "ukazka.yaml".into(),
        };
        let q = Recipe::from_yaml(&r.to_yaml(), "ukazka.yaml").unwrap();
        assert_eq!(r, q);
    }

    #[test]
    fn var_names() {
        assert!(valid_var_name("projekt_cesta"));
        assert!(valid_var_name("a1"));
        assert!(!valid_var_name(""));
        assert!(!valid_var_name("Projekt"));
        assert!(!valid_var_name("a b"));
    }
}
