//! File-based library: prompts/*.md, recipes/*.yaml, order.json.

use crate::json::Json;
use crate::model::{Prompt, Recipe};
use crate::render;
use crate::util::{new_id, now_iso, slugify};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct Library {
    pub root: PathBuf,
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Parse(String, String), // (file, message)
    NotFound(String),
    Invalid(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "IO error: {}", e),
            StoreError::Parse(file, m) => write!(f, "Cannot parse {}: {}", file, m),
            StoreError::NotFound(id) => write!(f, "Not found: {}", id),
            StoreError::Invalid(m) => write!(f, "Invalid: {}", m),
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl Library {
    pub fn open(root: impl Into<PathBuf>) -> Result<Library> {
        let lib = Library { root: root.into() };
        fs::create_dir_all(lib.prompts_dir())?;
        fs::create_dir_all(lib.recipes_dir())?;
        Ok(lib)
    }

    pub fn prompts_dir(&self) -> PathBuf {
        self.root.join("prompts")
    }
    pub fn recipes_dir(&self) -> PathBuf {
        self.root.join("recipes")
    }
    pub fn order_file(&self) -> PathBuf {
        self.root.join("order.json")
    }
    pub fn variables_file(&self) -> PathBuf {
        self.root.join("variables.json")
    }

    // ---------- prompts ----------

    /// All prompts sorted by order.json (unknown ids go last, alphabetically).
    pub fn list_prompts(&self) -> Result<Vec<Prompt>> {
        let mut prompts = Vec::new();
        for entry in fs::read_dir(self.prompts_dir())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = file_name(&path);
            // A single unreadable/corrupt file must not blank the whole
            // library (the UI would show no prompts although the rest is
            // fine) - skip it and keep going.
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("skipping unreadable prompt {}: {}", name, e);
                    continue;
                }
            };
            match Prompt::from_markdown(&text, &name) {
                Ok(p) => prompts.push(p),
                Err(e) => {
                    eprintln!("skipping corrupt prompt {}: {}", name, e);
                    continue;
                }
            }
        }
        let order = self.load_order()?;
        let pos: BTreeMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        prompts.sort_by(|a, b| {
            match (pos.get(a.id.as_str()), pos.get(b.id.as_str())) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a
                    .title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then(a.id.cmp(&b.id)),
            }
        });
        Ok(prompts)
    }

    pub fn get_prompt(&self, id: &str) -> Result<Prompt> {
        self.list_prompts()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn create_prompt(&self, title: &str, content: &str) -> Result<Prompt> {
        let now = now_iso();
        let mut p = Prompt {
            id: new_id("p"),
            title: title.to_string(),
            created: now.clone(),
            updated: now,
            recipes: vec![],
            selected_recipe: None,
            tags: vec![],
            content: content.to_string(),
            file_name: String::new(),
        };
        p.file_name = self.free_file_name(&self.prompts_dir(), &slugify(title), "md");
        self.write_prompt(&p)?;
        // append to order
        let mut order = self.load_order()?;
        order.push(p.id.clone());
        self.save_order(&order)?;
        Ok(p)
    }

    /// Updates an existing prompt (matched by id). Renames the file if the
    /// title changed enough to produce a different slug? No – file name is
    /// stable after creation to keep sync tools happy.
    pub fn save_prompt(&self, updated: &Prompt) -> Result<Prompt> {
        let existing = self.get_prompt(&updated.id)?;
        let mut p = updated.clone();
        p.file_name = existing.file_name.clone();
        p.created = if existing.created.is_empty() {
            now_iso()
        } else {
            existing.created.clone()
        };
        p.updated = now_iso();
        // drop references to recipes that no longer exist
        let known: Vec<String> = self.list_recipes()?.into_iter().map(|r| r.id).collect();
        p.recipes.retain(|r| known.contains(r));
        if let Some(sel) = &p.selected_recipe {
            if !p.recipes.contains(sel) {
                p.selected_recipe = p.recipes.first().cloned();
            }
        }
        self.write_prompt(&p)?;
        Ok(p)
    }

    pub fn delete_prompt(&self, id: &str) -> Result<()> {
        let p = self.get_prompt(id)?;
        fs::remove_file(self.prompts_dir().join(&p.file_name))?;
        let mut order = self.load_order()?;
        order.retain(|x| x != id);
        self.save_order(&order)?;
        Ok(())
    }

    pub fn duplicate_prompt(&self, id: &str) -> Result<Prompt> {
        let src = self.get_prompt(id)?;
        let copy = self.create_prompt(&format!("{} (kopie)", src.title), &src.content)?;
        let mut copy2 = copy.clone();
        copy2.recipes = src.recipes.clone();
        copy2.selected_recipe = src.selected_recipe.clone();
        self.save_prompt(&copy2)
    }

    fn write_prompt(&self, p: &Prompt) -> Result<()> {
        atomic_write(&self.prompts_dir().join(&p.file_name), p.to_markdown().as_bytes())?;
        Ok(())
    }

    // ---------- order ----------

    pub fn load_order(&self) -> Result<Vec<String>> {
        let path = self.order_file();
        if !path.exists() {
            return Ok(vec![]);
        }
        let text = fs::read_to_string(&path)?;
        let v = Json::parse(&text)
            .map_err(|e| StoreError::Parse("order.json".into(), e))?;
        Ok(v.get("prompts")
            .and_then(|a| a.as_arr())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn save_order(&self, ids: &[String]) -> Result<()> {
        let mut o = BTreeMap::new();
        o.insert(
            "prompts".to_string(),
            Json::Arr(ids.iter().map(|i| Json::str(i)).collect()),
        );
        atomic_write(&self.order_file(), Json::Obj(o).pretty().as_bytes())?;
        Ok(())
    }

    /// Replaces the order with the given id list (ids not present in the
    /// library are dropped; missing ids are appended in current order).
    pub fn reorder_prompts(&self, ids: &[String]) -> Result<Vec<String>> {
        let current: Vec<String> = self.list_prompts()?.into_iter().map(|p| p.id).collect();
        let mut order: Vec<String> = ids
            .iter()
            .filter(|i| current.contains(i))
            .cloned()
            .collect();
        for id in current {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        self.save_order(&order)?;
        Ok(order)
    }

    // ---------- recipes ----------

    pub fn list_recipes(&self) -> Result<Vec<Recipe>> {
        let mut recipes = Vec::new();
        for entry in fs::read_dir(self.recipes_dir())? {
            let entry = entry?;
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some("yaml") && ext != Some("yml") {
                continue;
            }
            let name = file_name(&path);
            // same leniency as list_prompts: one bad file must not blank all
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("skipping unreadable recipe {}: {}", name, e);
                    continue;
                }
            };
            match Recipe::from_yaml(&text, &name) {
                Ok(r) => recipes.push(r),
                Err(e) => {
                    eprintln!("skipping corrupt recipe {}: {}", name, e);
                    continue;
                }
            }
        }
        recipes.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(recipes)
    }

    pub fn get_recipe(&self, id: &str) -> Result<Recipe> {
        self.list_recipes()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn create_recipe(&self, name: &str) -> Result<Recipe> {
        // a new recipe starts out holding every known variable (empty), so it
        // is never structurally out of band
        let values = self
            .reconcile()?
            .into_iter()
            .map(|n| (n, String::new()))
            .collect();
        let mut r = Recipe {
            id: new_id("r"),
            name: name.to_string(),
            updated: now_iso(),
            values,
            file_name: String::new(),
        };
        r.file_name = self.free_file_name(&self.recipes_dir(), &slugify(name), "yaml");
        self.write_recipe(&r)?;
        Ok(r)
    }

    pub fn save_recipe(&self, updated: &Recipe) -> Result<Recipe> {
        let existing = self.get_recipe(&updated.id)?;
        let mut r = updated.clone();
        r.file_name = existing.file_name;
        r.updated = now_iso();
        for (k, _) in &r.values {
            if !crate::model::valid_var_name(k) {
                return Err(StoreError::Invalid(format!(
                    "invalid variable name '{}' (allowed: a-z, 0-9, _)",
                    k
                )));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (k, _) in &r.values {
            if !seen.insert(k.clone()) {
                return Err(StoreError::Invalid(format!("duplicate variable '{}'", k)));
            }
        }
        // Validation above runs FIRST so a rejected key can never leak into
        // variables.json. Only then is the recipe normalized to the global
        // variable set (a caller may legitimately introduce a new variable).
        let mut list = self.reconcile()?;
        let mut grew = false;
        for (k, _) in &r.values {
            if !list.contains(k) {
                list.push(k.clone());
                grew = true;
            }
        }
        r.values = normalized_values(&r, &list);
        self.write_recipe(&r)?;
        if grew {
            self.save_variables(&list)?;
            // the new variable has to reach every other recipe too
            for other in self.list_recipes()? {
                if other.id == r.id {
                    continue;
                }
                let desired = normalized_values(&other, &list);
                if desired != other.values {
                    let mut o2 = other.clone();
                    o2.values = desired;
                    self.write_recipe(&o2)?;
                }
            }
        }
        Ok(r)
    }

    /// Deletes a recipe and removes references from prompts.
    /// Returns titles of prompts that referenced it.
    pub fn delete_recipe(&self, id: &str) -> Result<Vec<String>> {
        let r = self.get_recipe(id)?;
        let mut affected = Vec::new();
        for p in self.list_prompts()? {
            if p.recipes.iter().any(|x| x == id) {
                affected.push(p.title.clone());
                let mut q = p.clone();
                q.recipes.retain(|x| x != id);
                if q.selected_recipe.as_deref() == Some(id) {
                    q.selected_recipe = q.recipes.first().cloned();
                }
                self.write_prompt(&q)?;
            }
        }
        fs::remove_file(self.recipes_dir().join(&r.file_name))?;
        Ok(affected)
    }

    /// Prompt titles that reference the recipe (for delete confirmation).
    pub fn recipe_usage(&self, id: &str) -> Result<Vec<String>> {
        Ok(self
            .list_prompts()?
            .into_iter()
            .filter(|p| p.recipes.iter().any(|x| x == id))
            .map(|p| p.title)
            .collect())
    }

    fn write_recipe(&self, r: &Recipe) -> Result<()> {
        atomic_write(&self.recipes_dir().join(&r.file_name), r.to_yaml().as_bytes())?;
        Ok(())
    }

    // ---------- global variables ----------
    //
    // Variables are first-class: an ordered list in variables.json. The
    // invariant is that EVERY recipe holds EVERY variable (value may be
    // empty). Reconciliation rule, applied everywhere:
    //
    //   order      comes from variables.json
    //   membership is the union of variables.json and all recipe keys
    //
    // A name found in a recipe but not in the file is appended (self-heal,
    // so a hand-edited library is never wrong); a name in the file but in no
    // recipe is kept (it is either brand new or empty everywhere). Nothing
    // is ever silently dropped.

    fn load_variables_raw(&self) -> Result<Option<Vec<String>>> {
        let path = self.variables_file();
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let v = Json::parse(&text).map_err(|e| StoreError::Parse("variables.json".into(), e))?;
        Ok(Some(
            v.get("variables")
                .and_then(|a| a.as_arr())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        ))
    }

    fn save_variables(&self, names: &[String]) -> Result<()> {
        let mut o = BTreeMap::new();
        o.insert(
            "variables".to_string(),
            Json::Arr(names.iter().map(|n| Json::str(n)).collect()),
        );
        atomic_write(&self.variables_file(), Json::Obj(o).pretty().as_bytes())?;
        Ok(())
    }

    /// Applies the reconciliation rule in memory. Never writes, so it is safe
    /// to call on every read (and keeps the UI correct even when another
    /// process changed a recipe a moment ago).
    fn reconcile(&self) -> Result<Vec<String>> {
        let file = self.load_variables_raw()?;
        let mut list: Vec<String> = Vec::new();
        if let Some(f) = file {
            for n in f {
                if crate::model::valid_var_name(&n) && !list.contains(&n) {
                    list.push(n);
                }
            }
        }
        for r in self.list_recipes()? {
            for (k, _) in &r.values {
                if crate::model::valid_var_name(k) && !list.contains(k) {
                    list.push(k.clone());
                }
            }
        }
        Ok(list)
    }

    /// The ordered global variable names.
    pub fn list_variables(&self) -> Result<Vec<String>> {
        self.reconcile()
    }

    /// Global variables in order, each with the value held by every recipe.
    pub fn variables(&self) -> Result<Vec<(String, Vec<(String, String, String)>)>> {
        let names = self.reconcile()?;
        let recipes = self.list_recipes()?;
        Ok(names
            .into_iter()
            .map(|n| {
                let uses = recipes
                    .iter()
                    .map(|r| {
                        let val = value_of(r, &n).unwrap_or_default();
                        (r.id.clone(), r.name.clone(), val)
                    })
                    .collect();
                (n, uses)
            })
            .collect())
    }

    /// Titles of prompts whose body still uses `{{name}}` (delete confirmation).
    pub fn variable_usage(&self, name: &str) -> Result<Vec<String>> {
        Ok(self
            .list_prompts()?
            .into_iter()
            .filter(|p| render::placeholders(&p.content).iter().any(|x| x == name))
            .map(|p| p.title)
            .collect())
    }

    /// Brings variables.json and the recipes in line with each other.
    ///
    /// Idempotent by construction: nothing is written unless it actually
    /// changes. That is mandatory, not cosmetic - the app polls
    /// `fingerprint()` every 1.5 s, so an unconditional write here would turn
    /// into an endless refresh loop across every window and process.
    /// Call it once per process, never from the watcher.
    pub fn migrate(&self) -> Result<Vec<String>> {
        let file = self.load_variables_raw()?;
        let list = self.reconcile()?;
        for r in self.list_recipes()? {
            let desired = normalized_values(&r, &list);
            if desired != r.values {
                let mut r2 = r.clone();
                r2.values = desired;
                // deliberately not save_recipe: a structural backfill is not a
                // user edit and must not bump `updated`
                self.write_recipe(&r2)?;
            }
        }
        if file.as_deref() != Some(list.as_slice()) {
            self.save_variables(&list)?;
        }
        Ok(list)
    }

    /// Adds a variable to the global list and to every recipe (empty value).
    pub fn add_variable(&self, name: &str) -> Result<Vec<String>> {
        if !crate::model::valid_var_name(name) {
            return Err(StoreError::Invalid(format!(
                "invalid variable name '{}' (allowed: a-z, 0-9, _)",
                name
            )));
        }
        let mut list = self.reconcile()?;
        if list.iter().any(|n| n == name) {
            return Err(StoreError::Invalid(format!(
                "variable '{}' already exists",
                name
            )));
        }
        list.push(name.to_string());
        // recipes first, then the file: a crash in between leaves the key in
        // the recipes and self-heal finishes the job on the next read
        for r in self.list_recipes()? {
            if value_of(&r, name).is_none() {
                let mut r2 = r.clone();
                r2.values.push((name.to_string(), String::new()));
                self.write_recipe(&r2)?;
            }
        }
        self.save_variables(&list)?;
        Ok(list)
    }

    /// Renames a variable everywhere: the key in every recipe (value kept)
    /// and `{{old}}` -> `{{new}}` in every prompt body.
    pub fn rename_variable(&self, old: &str, new: &str) -> Result<Vec<String>> {
        if old == new {
            return self.list_variables();
        }
        if !crate::model::valid_var_name(new) {
            return Err(StoreError::Invalid(format!(
                "invalid variable name '{}' (allowed: a-z, 0-9, _)",
                new
            )));
        }
        let mut list = self.reconcile()?;
        if !list.iter().any(|n| n == old) {
            return Err(StoreError::NotFound(old.to_string()));
        }
        if list.iter().any(|n| n == new) {
            return Err(StoreError::Invalid(format!(
                "variable '{}' already exists",
                new
            )));
        }
        for r in self.list_recipes()? {
            if value_of(&r, old).is_some() {
                let mut r2 = r.clone();
                for (k, _) in r2.values.iter_mut() {
                    if k == old {
                        *k = new.to_string();
                    }
                }
                self.write_recipe(&r2)?;
            }
        }
        for p in self.list_prompts()? {
            let content = render::rename_placeholder(&p.content, old, new);
            if content != p.content {
                let mut q = p.clone();
                q.content = content;
                q.updated = now_iso(); // the body really did change
                self.write_prompt(&q)?;
            }
        }
        for n in list.iter_mut() {
            if n == old {
                *n = new.to_string();
            }
        }
        self.save_variables(&list)?;
        Ok(list)
    }

    /// Removes a variable from the global list and from every recipe.
    /// Prompt bodies are left alone - `{{name}}` simply becomes missing.
    pub fn delete_variable(&self, name: &str) -> Result<Vec<String>> {
        let mut list = self.reconcile()?;
        if !list.iter().any(|n| n == name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        // strip from recipes BEFORE the file, otherwise a crash in between
        // lets self-heal resurrect the name from a recipe that still has it
        for r in self.list_recipes()? {
            if value_of(&r, name).is_some() {
                let mut r2 = r.clone();
                r2.values.retain(|(k, _)| k != name);
                self.write_recipe(&r2)?;
            }
        }
        list.retain(|n| n != name);
        self.save_variables(&list)?;
        Ok(list)
    }

    // ---------- render ----------

    pub fn render_prompt(
        &self,
        prompt_id: &str,
        recipe_id: Option<&str>,
        overrides: &BTreeMap<String, String>,
    ) -> Result<render::RenderResult> {
        let p = self.get_prompt(prompt_id)?;
        let recipe = match recipe_id {
            Some(rid) => Some(self.get_recipe(rid)?),
            None => None,
        };
        let mut values: BTreeMap<&str, &str> = match &recipe {
            Some(r) => r.values_map(),
            None => BTreeMap::new(),
        };
        for (k, v) in overrides {
            values.insert(k.as_str(), v.as_str());
        }
        if recipe.is_none() && overrides.is_empty() {
            // raw copy: no substitution at all
            return Ok(render::RenderResult {
                text: p.content.clone(),
                missing: vec![],
            });
        }
        Ok(render::render(&p.content, &values))
    }

    // ---------- misc ----------

    fn free_file_name(&self, dir: &Path, slug: &str, ext: &str) -> String {
        let mut name = format!("{}.{}", slug, ext);
        let mut n = 1;
        while dir.join(&name).exists() {
            n += 1;
            name = format!("{}-{}.{}", slug, n, ext);
        }
        name
    }

    /// Cheap fingerprint of the library for the polling watcher:
    /// hashes paths + modification times + sizes.
    pub fn fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix_bytes = |h: &mut u64, bytes: &[u8]| {
            for b in bytes {
                *h ^= *b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
        };
        let walk = |dir: &Path, h: &mut u64| {
            if let Ok(rd) = fs::read_dir(dir) {
                let mut entries: Vec<_> = rd.flatten().collect();
                entries.sort_by_key(|e| e.file_name());
                for e in entries {
                    mix_bytes(h, e.file_name().to_string_lossy().as_bytes());
                    if let Ok(md) = e.metadata() {
                        mix_bytes(h, &md.len().to_le_bytes());
                        if let Ok(t) = md.modified() {
                            if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                                mix_bytes(h, &d.as_nanos().to_le_bytes());
                            }
                        }
                    }
                }
            }
        };
        walk(&self.prompts_dir(), &mut h);
        walk(&self.recipes_dir(), &mut h);
        for f in [self.order_file(), self.variables_file()] {
            if let Ok(md) = fs::metadata(f) {
                mix_bytes(&mut h, &md.len().to_le_bytes());
                if let Ok(t) = md.modified() {
                    if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                        mix_bytes(&mut h, &d.as_nanos().to_le_bytes());
                    }
                }
            }
        }
        h
    }

    /// Creates a couple of sample prompts/recipes on first run.
    pub fn seed_if_empty(&self) -> Result<bool> {
        if !self.list_prompts()?.is_empty() || !self.list_recipes()?.is_empty() {
            return Ok(false);
        }
        let mut r1 = self.create_recipe("Ukázkový projekt")?;
        r1.values = vec![
            ("projekt_cesta".into(), "C:\\Projekty\\MujProjekt".into()),
            ("projekt_nazev".into(), "MujProjekt".into()),
            ("jazyk".into(), "rust".into()),
        ];
        let r1 = self.save_recipe(&r1)?;
        let mut r2 = self.create_recipe("Obecný")?;
        // every variable gets a value here on purpose: with "empty == missing"
        // a blank would make the very first launch look broken
        r2.values = vec![
            ("projekt_cesta".into(), "C:\\tmp".into()),
            ("projekt_nazev".into(), "Projekt".into()),
            ("jazyk".into(), "python".into()),
        ];
        let r2 = self.save_recipe(&r2)?;

        let p1 = self.create_prompt(
            "Code review",
            "Proveď důkladné code review projektu ve složce {{projekt_cesta}}.\nJazyk projektu: {{jazyk}}.\nZaměř se na bezpečnost, čitelnost a výkon. Shrň 5 hlavních nálezů.",
        )?;
        let mut p1b = p1.clone();
        p1b.recipes = vec![r1.id.clone(), r2.id.clone()];
        p1b.selected_recipe = Some(r1.id.clone());
        self.save_prompt(&p1b)?;

        self.create_prompt(
            "Sumarizace textu",
            "Shrň následující text do pěti odrážek a jedné věty závěru:\n\n",
        )?;
        Ok(true)
    }
}

fn value_of(r: &Recipe, name: &str) -> Option<String> {
    r.values
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

/// The recipe's values re-stated in global order, every variable present.
/// Keys the global list does not know (e.g. an invalid name typed straight
/// into the YAML by hand) are preserved at the end rather than dropped.
fn normalized_values(r: &Recipe, list: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = list
        .iter()
        .map(|n| (n.clone(), value_of(r, n).unwrap_or_default()))
        .collect();
    for (k, v) in &r.values {
        if !list.iter().any(|n| n == k) {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp~");
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows: rename over existing file can fail -> remove + rename
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_lib(name: &str) -> Library {
        let dir = std::env::temp_dir().join(format!(
            "pm-test-{}-{}",
            name,
            crate::util::new_id("t")
        ));
        let _ = fs::remove_dir_all(&dir);
        Library::open(dir).unwrap()
    }

    #[test]
    fn crud_prompt_and_order() {
        let lib = tmp_lib("crud");
        let a = lib.create_prompt("Beta", "obsah B {{x}}").unwrap();
        let b = lib.create_prompt("Alfa", "obsah A").unwrap();
        // creation order preserved via order.json
        let list = lib.list_prompts().unwrap();
        assert_eq!(list[0].id, a.id);
        assert_eq!(list[1].id, b.id);
        // reorder
        lib.reorder_prompts(&[b.id.clone(), a.id.clone()]).unwrap();
        let list = lib.list_prompts().unwrap();
        assert_eq!(list[0].id, b.id);
        // update
        let mut e = list[1].clone();
        e.title = "Beta 2".into();
        e.content = "nový obsah".into();
        let saved = lib.save_prompt(&e).unwrap();
        assert_eq!(saved.title, "Beta 2");
        assert_eq!(saved.file_name, a.file_name, "file name must be stable");
        assert!(!saved.updated.is_empty());
        // delete
        lib.delete_prompt(&a.id).unwrap();
        assert_eq!(lib.list_prompts().unwrap().len(), 1);
        assert_eq!(lib.load_order().unwrap(), vec![b.id.clone()]);
    }

    #[test]
    fn slug_collisions() {
        let lib = tmp_lib("slug");
        let a = lib.create_prompt("Stejný název", "1").unwrap();
        let b = lib.create_prompt("Stejný název", "2").unwrap();
        assert_eq!(a.file_name, "stejny-nazev.md");
        assert_eq!(b.file_name, "stejny-nazev-2.md");
    }

    #[test]
    fn recipes_crud_and_refs() {
        let lib = tmp_lib("recipes");
        let r = lib.create_recipe("Recept A").unwrap();
        let mut r2 = r.clone();
        r2.values = vec![("cesta".into(), "C:\\X".into())];
        lib.save_recipe(&r2).unwrap();

        let p = lib.create_prompt("P", "{{cesta}} a {{jine}}").unwrap();
        let mut p2 = p.clone();
        p2.recipes = vec![r.id.clone()];
        p2.selected_recipe = Some(r.id.clone());
        lib.save_prompt(&p2).unwrap();

        assert_eq!(lib.recipe_usage(&r.id).unwrap(), vec!["P".to_string()]);

        // delete recipe -> reference and selection cleaned
        let affected = lib.delete_recipe(&r.id).unwrap();
        assert_eq!(affected, vec!["P".to_string()]);
        let p3 = lib.get_prompt(&p.id).unwrap();
        assert!(p3.recipes.is_empty());
        assert_eq!(p3.selected_recipe, None);
    }

    #[test]
    fn save_prompt_drops_dead_recipe_refs() {
        let lib = tmp_lib("deadrefs");
        let p = lib.create_prompt("P", "x").unwrap();
        let mut p2 = p.clone();
        p2.recipes = vec!["r-neexistuje".into()];
        p2.selected_recipe = Some("r-neexistuje".into());
        let saved = lib.save_prompt(&p2).unwrap();
        assert!(saved.recipes.is_empty());
        assert_eq!(saved.selected_recipe, None);
    }

    #[test]
    fn variables_are_global_and_ordered() {
        let lib = tmp_lib("vars");
        let mut a = lib.create_recipe("A").unwrap();
        a.values = vec![("x".into(), "1".into()), ("y".into(), "2".into())];
        lib.save_recipe(&a).unwrap();
        let mut b = lib.create_recipe("B").unwrap();
        b.values = vec![("x".into(), "9".into())]; // y omitted on purpose
        lib.save_recipe(&b).unwrap();

        assert_eq!(lib.list_variables().unwrap(), vec!["x", "y"]);
        let vars = lib.variables().unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].0, "x", "order comes from variables.json, not the alphabet");
        // every variable now lists EVERY recipe
        assert_eq!(vars[0].1.len(), 2);
        assert_eq!(vars[1].1.len(), 2);
        // the omitted key was backfilled as empty rather than lost
        assert_eq!(value_of(&lib.get_recipe(&b.id).unwrap(), "y"), Some(String::new()));
    }

    #[test]
    fn variables_survive_zero_recipes() {
        let lib = tmp_lib("norecipes");
        lib.add_variable("foo").unwrap();
        assert_eq!(lib.list_variables().unwrap(), vec!["foo"]);
    }

    #[test]
    fn new_recipe_gets_all_variables() {
        let lib = tmp_lib("newrec");
        lib.add_variable("a").unwrap();
        lib.add_variable("b").unwrap();
        let r = lib.create_recipe("R").unwrap();
        let keys: Vec<String> = r.values.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn save_recipe_extends_global_list_and_backfills_others() {
        let lib = tmp_lib("extend");
        let a = lib.create_recipe("A").unwrap();
        let b = lib.create_recipe("B").unwrap();
        let mut a2 = a.clone();
        a2.values = vec![("nova".into(), "1".into())];
        lib.save_recipe(&a2).unwrap();

        assert_eq!(lib.list_variables().unwrap(), vec!["nova"]);
        assert_eq!(
            value_of(&lib.get_recipe(&b.id).unwrap(), "nova"),
            Some(String::new()),
            "a variable introduced through one recipe must reach the others"
        );
    }

    #[test]
    fn migrate_backfills_recipes_and_is_idempotent() {
        let lib = tmp_lib("migrate");
        // simulate a v1 library: recipes with different key subsets, no variables.json
        let r1 = Recipe {
            id: "r-1".into(),
            name: "A".into(),
            updated: "t".into(),
            values: vec![("x".into(), "1".into()), ("y".into(), "2".into())],
            file_name: "a.yaml".into(),
        };
        let r2 = Recipe {
            id: "r-2".into(),
            name: "B".into(),
            updated: "t".into(),
            values: vec![("z".into(), "3".into())],
            file_name: "b.yaml".into(),
        };
        fs::write(lib.recipes_dir().join("a.yaml"), r1.to_yaml()).unwrap();
        fs::write(lib.recipes_dir().join("b.yaml"), r2.to_yaml()).unwrap();
        assert!(!lib.variables_file().exists());

        let list = lib.migrate().unwrap();
        assert_eq!(list, vec!["x", "y", "z"]);
        for r in lib.list_recipes().unwrap() {
            assert_eq!(r.values.len(), 3, "every recipe must carry every variable");
        }
        let b = lib.get_recipe("r-2").unwrap();
        assert_eq!(value_of(&b, "x"), Some(String::new()));
        assert_eq!(value_of(&b, "z"), Some("3".to_string()), "values are kept");

        // Idempotence is the whole ballgame: the app polls fingerprint() every
        // 1.5 s, so a second migrate that rewrites files = endless refresh loop.
        let f1 = lib.fingerprint();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(lib.migrate().unwrap(), list);
        assert_eq!(f1, lib.fingerprint(), "migrate must not rewrite anything");
    }

    #[test]
    fn rename_variable_rewrites_recipes_and_prompts() {
        let lib = tmp_lib("rename");
        let mut r = lib.create_recipe("R").unwrap();
        r.values = vec![("stary".into(), "hodnota".into())];
        let r = lib.save_recipe(&r).unwrap();
        let p = lib.create_prompt("P", "A {{stary}} B \\{{stary}}").unwrap();

        lib.rename_variable("stary", "novy").unwrap();

        assert_eq!(lib.list_variables().unwrap(), vec!["novy"]);
        assert_eq!(
            value_of(&lib.get_recipe(&r.id).unwrap(), "novy"),
            Some("hodnota".to_string()),
            "the value must survive the rename"
        );
        assert_eq!(
            lib.get_prompt(&p.id).unwrap().content,
            "A {{novy}} B \\{{stary}}",
            "the escaped form must stay untouched"
        );
        assert!(matches!(
            lib.rename_variable("neexistuje", "x"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn delete_variable_keeps_prompt_bodies() {
        let lib = tmp_lib("delvar");
        let mut r = lib.create_recipe("R").unwrap();
        r.values = vec![("a".into(), "A".into()), ("b".into(), "B".into())];
        let r = lib.save_recipe(&r).unwrap();
        let p = lib.create_prompt("P", "{{a}} {{b}}").unwrap();
        assert_eq!(lib.variable_usage("a").unwrap(), vec!["P".to_string()]);

        lib.delete_variable("a").unwrap();

        assert_eq!(lib.list_variables().unwrap(), vec!["b"]);
        assert!(value_of(&lib.get_recipe(&r.id).unwrap(), "a").is_none());
        assert_eq!(
            lib.get_prompt(&p.id).unwrap().content,
            "{{a}} {{b}}",
            "prompt bodies are deliberately left alone"
        );
        // the orphaned placeholder simply reports as missing
        let out = lib.render_prompt(&p.id, Some(&r.id), &BTreeMap::new()).unwrap();
        assert_eq!(out.text, "{{a}} B");
        assert_eq!(out.missing, vec!["a"]);
    }

    #[test]
    fn empty_recipe_value_renders_as_missing() {
        let lib = tmp_lib("emptyval");
        let mut r = lib.create_recipe("R").unwrap();
        r.values = vec![("a".into(), "".into()), ("b".into(), "B".into())];
        let r = lib.save_recipe(&r).unwrap();
        let p = lib.create_prompt("P", "{{a}}/{{b}}").unwrap();
        let out = lib.render_prompt(&p.id, Some(&r.id), &BTreeMap::new()).unwrap();
        assert_eq!(out.text, "{{a}}/B");
        assert_eq!(out.missing, vec!["a"]);
    }

    #[test]
    fn render_with_recipe_and_overrides() {
        let lib = tmp_lib("render");
        let mut r = lib.create_recipe("R").unwrap();
        r.values = vec![("a".into(), "AAA".into())];
        let r = lib.save_recipe(&r).unwrap();
        let p = lib.create_prompt("P", "{{a}} {{b}}").unwrap();

        let out = lib.render_prompt(&p.id, Some(&r.id), &BTreeMap::new()).unwrap();
        assert_eq!(out.text, "AAA {{b}}");
        assert_eq!(out.missing, vec!["b"]);

        let mut ov = BTreeMap::new();
        ov.insert("b".to_string(), "BBB".to_string());
        let out = lib.render_prompt(&p.id, Some(&r.id), &ov).unwrap();
        assert_eq!(out.text, "AAA BBB");
        assert!(out.missing.is_empty());

        // raw
        let out = lib.render_prompt(&p.id, None, &BTreeMap::new()).unwrap();
        assert_eq!(out.text, "{{a}} {{b}}");
    }

    #[test]
    fn invalid_recipe_values_rejected() {
        let lib = tmp_lib("invalid");
        let r = lib.create_recipe("R").unwrap();
        let mut bad = r.clone();
        bad.values = vec![("Špatný Klíč".into(), "x".into())];
        assert!(matches!(
            lib.save_recipe(&bad),
            Err(StoreError::Invalid(_))
        ));
        let mut dup = r.clone();
        dup.values = vec![("a".into(), "1".into()), ("a".into(), "2".into())];
        assert!(matches!(lib.save_recipe(&dup), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn seed_and_fingerprint() {
        let lib = tmp_lib("seed");
        assert!(lib.seed_if_empty().unwrap());
        assert!(!lib.seed_if_empty().unwrap());
        let f1 = lib.fingerprint();
        let p = &lib.list_prompts().unwrap()[0];
        let mut p2 = p.clone();
        p2.content.push_str("\nzměna");
        std::thread::sleep(std::time::Duration::from_millis(30));
        lib.save_prompt(&p2).unwrap();
        assert_ne!(f1, lib.fingerprint());
    }

    #[test]
    fn corrupt_file_does_not_blank_the_library() {
        let lib = tmp_lib("corrupt");
        lib.create_prompt("Dobrý", "obsah").unwrap();
        let r = lib.create_recipe("Recept").unwrap();
        // a torn/garbage file next to the healthy ones
        fs::write(lib.prompts_dir().join("torn.md"), "---\nid: bez konce").unwrap();
        fs::write(lib.recipes_dir().join("torn.yaml"), "\u{0}\u{0}\u{0}").unwrap();
        let prompts = lib.list_prompts().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "Dobrý");
        let recipes = lib.list_recipes().unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].id, r.id);
    }

    #[test]
    fn portable_copy_of_library() {
        let lib = tmp_lib("copy-src");
        lib.seed_if_empty().unwrap();
        let order = lib.load_order().unwrap();
        // simulate copying the folder to another machine
        let dst = std::env::temp_dir().join(format!("pm-test-copy-dst-{}", new_id("t")));
        copy_dir(&lib.root, &dst).unwrap();
        let lib2 = Library::open(&dst).unwrap();
        assert_eq!(
            lib.list_prompts().unwrap().len(),
            lib2.list_prompts().unwrap().len()
        );
        assert_eq!(lib2.load_order().unwrap(), order);
        assert_eq!(
            lib.list_recipes().unwrap().len(),
            lib2.list_recipes().unwrap().len()
        );
    }

    fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            let to = dst.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &to)?;
            } else {
                fs::copy(e.path(), &to)?;
            }
        }
        Ok(())
    }
}
