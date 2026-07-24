//! MCP protocol implementation (library part, unit-testable).

use pm_core::json::Json;
use pm_core::storage::StoreError;
use pm_core::Library;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "prompt-manager";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default library root, kept in sync with the desktop app:
/// reads settings.json if present, otherwise the platform default.
pub fn default_library_root() -> PathBuf {
    if let Ok(env) = std::env::var("PROMPT_MANAGER_LIBRARY") {
        if !env.trim().is_empty() {
            return PathBuf::from(env);
        }
    }
    let config_dir = config_dir();
    let settings = config_dir.join("settings.json");
    if let Ok(text) = std::fs::read_to_string(&settings) {
        if let Ok(v) = Json::parse(&text) {
            if let Some(dir) = v.get_str("library_dir") {
                if !dir.trim().is_empty() {
                    return PathBuf::from(dir);
                }
            }
        }
    }
    config_dir.join("library")
}

fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("PromptManager");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("PromptManager");
    }
    PathBuf::from(".").join("PromptManager")
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Json::Obj(m)
}

/// Handles one raw JSON-RPC message. Returns None for notifications.
pub fn handle_message(raw: &str, library_root: &Path) -> Option<String> {
    let msg = match Json::parse(raw) {
        Ok(m) => m,
        Err(e) => {
            return Some(
                rpc_error(Json::Null, -32700, &format!("parse error: {}", e)).dump(),
            )
        }
    };
    let id = msg.get("id").cloned().unwrap_or(Json::Null);
    let method = msg.get_str("method").unwrap_or("").to_string();
    let params = msg.get("params").cloned().unwrap_or(Json::Null);

    // notifications get no response
    if msg.get("id").is_none() {
        return None;
    }

    let result = match method.as_str() {
        "initialize" => Ok(obj(vec![
            ("protocolVersion", {
                // echo the client's version if we know it, else ours
                let v = params
                    .get_str("protocolVersion")
                    .unwrap_or(PROTOCOL_VERSION);
                Json::str(v)
            }),
            ("capabilities", obj(vec![("tools", obj(vec![]))])),
            (
                "serverInfo",
                obj(vec![
                    ("name", Json::str(SERVER_NAME)),
                    ("version", Json::str(SERVER_VERSION)),
                ]),
            ),
        ])),
        "ping" => Ok(obj(vec![])),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(&params, library_root),
        _ => Err((-32601i64, format!("method not found: {}", method))),
    };

    Some(match result {
        Ok(r) => obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", id),
            ("result", r),
        ])
        .dump(),
        Err((code, message)) => rpc_error(id, code, &message).dump(),
    })
}

fn rpc_error(id: Json, code: i64, message: &str) -> Json {
    obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        (
            "error",
            obj(vec![
                ("code", Json::Num(code as f64)),
                ("message", Json::str(message)),
            ]),
        ),
    ])
}

fn tool(name: &str, description: &str, schema: Json) -> Json {
    obj(vec![
        ("name", Json::str(name)),
        ("description", Json::str(description)),
        ("inputSchema", schema),
    ])
}

fn schema(props: Vec<(&str, &str, &str)>, required: &[&str]) -> Json {
    // props: (name, type, description)
    let mut p = BTreeMap::new();
    for (name, ty, desc) in props {
        p.insert(
            name.to_string(),
            obj(vec![("type", Json::str(ty)), ("description", Json::str(desc))]),
        );
    }
    obj(vec![
        ("type", Json::str("object")),
        ("properties", Json::Obj(p)),
        (
            "required",
            Json::Arr(required.iter().map(|r| Json::str(r)).collect()),
        ),
    ])
}

fn tools_list() -> Json {
    let tools = vec![
        tool(
            "list_prompts",
            "Vrátí seznam všech promptů (id, název, přiřazené recepty, placeholdery).",
            schema(vec![], &[]),
        ),
        tool(
            "get_prompt",
            "Vrátí celý prompt včetně obsahu (šablony).",
            schema(
                vec![("id", "string", "ID promptu (nebo přesný název)")],
                &["id"],
            ),
        ),
        tool(
            "render_prompt",
            "Vyrenderuje prompt: dosadí hodnoty zvoleného receptu do {{placeholderů}}. Vrací text a seznam nevyplněných placeholderů.",
            schema(
                vec![
                    ("id", "string", "ID promptu (nebo přesný název)"),
                    (
                        "recipe",
                        "string",
                        "ID nebo přesný název receptu; vynecháte-li, vrátí se surová šablona",
                    ),
                ],
                &["id"],
            ),
        ),
        tool(
            "create_prompt",
            "Vytvoří nový prompt.",
            schema(
                vec![
                    ("title", "string", "Název promptu"),
                    ("content", "string", "Obsah šablony s {{placeholdery}}"),
                ],
                &["title"],
            ),
        ),
        tool(
            "update_prompt",
            "Upraví existující prompt (název a/nebo obsah).",
            schema(
                vec![
                    ("id", "string", "ID promptu"),
                    ("title", "string", "Nový název (volitelné)"),
                    ("content", "string", "Nový obsah (volitelné)"),
                ],
                &["id"],
            ),
        ),
        tool(
            "delete_prompt",
            "Smaže prompt.",
            schema(vec![("id", "string", "ID promptu")], &["id"]),
        ),
        tool(
            "list_recipes",
            "Vrátí seznam všech receptů včetně jejich proměnných.",
            schema(vec![], &[]),
        ),
        tool(
            "get_recipe",
            "Vrátí jeden recept.",
            schema(
                vec![("id", "string", "ID receptu (nebo přesný název)")],
                &["id"],
            ),
        ),
        tool(
            "create_recipe",
            "Vytvoří nový recept. Hodnoty proměnných lze předat jako objekt values.",
            schema(
                vec![
                    ("name", "string", "Název receptu"),
                    (
                        "values",
                        "object",
                        "Proměnné receptu: {\"nazev_promenne\": \"hodnota\"}",
                    ),
                ],
                &["name"],
            ),
        ),
        tool(
            "update_recipe",
            "Upraví recept (název a/nebo proměnné).",
            schema(
                vec![
                    ("id", "string", "ID receptu"),
                    ("name", "string", "Nový název (volitelné)"),
                    ("values", "object", "Nové proměnné (volitelné, nahradí stávající)"),
                ],
                &["id"],
            ),
        ),
        tool(
            "list_variables",
            "Vrátí sjednocení proměnných definovaných napříč recepty.",
            schema(vec![], &[]),
        ),
    ];
    obj(vec![("tools", Json::Arr(tools))])
}

type RpcResult = Result<Json, (i64, String)>;

fn tools_call(params: &Json, root: &Path) -> RpcResult {
    let name = params
        .get_str("name")
        .ok_or((-32602i64, "missing tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or(Json::Null);
    let lib = Library::open(root).map_err(internal)?;
    match run_tool(name, &args, &lib) {
        Ok(v) => Ok(tool_ok(v)),
        Err(ToolError::User(msg)) => Ok(tool_err(&msg)),
        Err(ToolError::Rpc(code, msg)) => Err((code, msg)),
    }
}

enum ToolError {
    /// reported inside the tool result (isError: true)
    User(String),
    /// protocol-level error
    Rpc(i64, String),
}

fn internal(e: StoreError) -> (i64, String) {
    (-32603, format!("{}", e))
}

fn store(e: StoreError) -> ToolError {
    match e {
        StoreError::NotFound(id) => ToolError::User(format!("Nenalezeno: {}", id)),
        StoreError::Invalid(m) => ToolError::User(m),
        other => ToolError::User(format!("{}", other)),
    }
}

fn tool_ok(v: Json) -> Json {
    obj(vec![
        (
            "content",
            Json::Arr(vec![obj(vec![
                ("type", Json::str("text")),
                ("text", Json::str(&v.pretty())),
            ])]),
        ),
        ("isError", Json::Bool(false)),
    ])
}

fn tool_err(msg: &str) -> Json {
    obj(vec![
        (
            "content",
            Json::Arr(vec![obj(vec![
                ("type", Json::str("text")),
                ("text", Json::str(msg)),
            ])]),
        ),
        ("isError", Json::Bool(true)),
    ])
}

fn find_prompt_id(lib: &Library, id_or_title: &str) -> Result<String, ToolError> {
    let prompts = lib.list_prompts().map_err(store)?;
    if let Some(p) = prompts.iter().find(|p| p.id == id_or_title) {
        return Ok(p.id.clone());
    }
    if let Some(p) = prompts.iter().find(|p| p.title == id_or_title) {
        return Ok(p.id.clone());
    }
    Err(ToolError::User(format!(
        "Prompt '{}' nenalezen. Použijte list_prompts.",
        id_or_title
    )))
}

fn find_recipe_id(lib: &Library, id_or_name: &str) -> Result<String, ToolError> {
    let recipes = lib.list_recipes().map_err(store)?;
    if let Some(r) = recipes.iter().find(|r| r.id == id_or_name) {
        return Ok(r.id.clone());
    }
    if let Some(r) = recipes.iter().find(|r| r.name == id_or_name) {
        return Ok(r.id.clone());
    }
    Err(ToolError::User(format!(
        "Recept '{}' nenalezen. Použijte list_recipes.",
        id_or_name
    )))
}

fn run_tool(name: &str, args: &Json, lib: &Library) -> Result<Json, ToolError> {
    match name {
        "list_prompts" => {
            let ps = lib.list_prompts().map_err(store)?;
            Ok(Json::Arr(ps.iter().map(|p| p.to_json()).collect()))
        }
        "get_prompt" => {
            let id = req_str(args, "id")?;
            let id = find_prompt_id(lib, &id)?;
            Ok(lib.get_prompt(&id).map_err(store)?.to_json())
        }
        "render_prompt" => {
            let id = req_str(args, "id")?;
            let id = find_prompt_id(lib, &id)?;
            let recipe = match args.get_str("recipe") {
                Some(r) if !r.trim().is_empty() => Some(find_recipe_id(lib, r)?),
                _ => None,
            };
            let out = lib
                .render_prompt(&id, recipe.as_deref(), &BTreeMap::new())
                .map_err(store)?;
            Ok(obj(vec![
                ("text", Json::str(&out.text)),
                (
                    "missing",
                    Json::Arr(out.missing.iter().map(|m| Json::str(m)).collect()),
                ),
            ]))
        }
        "create_prompt" => {
            let title = req_str(args, "title")?;
            let content = args.get_str("content").unwrap_or("");
            Ok(lib.create_prompt(&title, content).map_err(store)?.to_json())
        }
        "update_prompt" => {
            let id = req_str(args, "id")?;
            let id = find_prompt_id(lib, &id)?;
            let mut p = lib.get_prompt(&id).map_err(store)?;
            if let Some(t) = args.get_str("title") {
                p.title = t.to_string();
            }
            if let Some(c) = args.get_str("content") {
                p.content = c.to_string();
            }
            Ok(lib.save_prompt(&p).map_err(store)?.to_json())
        }
        "delete_prompt" => {
            let id = req_str(args, "id")?;
            let id = find_prompt_id(lib, &id)?;
            lib.delete_prompt(&id).map_err(store)?;
            Ok(obj(vec![("deleted", Json::Bool(true))]))
        }
        "list_recipes" => {
            let rs = lib.list_recipes().map_err(store)?;
            Ok(Json::Arr(rs.iter().map(|r| r.to_json()).collect()))
        }
        "get_recipe" => {
            let id = req_str(args, "id")?;
            let id = find_recipe_id(lib, &id)?;
            Ok(lib.get_recipe(&id).map_err(store)?.to_json())
        }
        "create_recipe" => {
            let name = req_str(args, "name")?;
            let mut r = lib.create_recipe(&name).map_err(store)?;
            if let Some(vals) = args.get("values").and_then(|v| v.as_obj()) {
                r.values = vals
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
                r = lib.save_recipe(&r).map_err(store)?;
            }
            Ok(r.to_json())
        }
        "update_recipe" => {
            let id = req_str(args, "id")?;
            let id = find_recipe_id(lib, &id)?;
            let mut r = lib.get_recipe(&id).map_err(store)?;
            if let Some(n) = args.get_str("name") {
                r.name = n.to_string();
            }
            if let Some(vals) = args.get("values").and_then(|v| v.as_obj()) {
                r.values = vals
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
            }
            Ok(lib.save_recipe(&r).map_err(store)?.to_json())
        }
        "list_variables" => {
            let vars = lib.variables().map_err(store)?;
            let mut arr = Vec::new();
            for (name, uses) in vars {
                arr.push(obj(vec![
                    ("name", Json::str(&name)),
                    (
                        "recipes",
                        Json::Arr(
                            uses.iter()
                                .map(|(rid, rname, val)| {
                                    obj(vec![
                                        ("recipe_id", Json::str(rid)),
                                        ("recipe_name", Json::str(rname)),
                                        ("value", Json::str(val)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ]));
            }
            Ok(Json::Arr(arr))
        }
        other => Err(ToolError::Rpc(
            -32602,
            format!("unknown tool: {}", other),
        )),
    }
}

fn req_str(args: &Json, key: &str) -> Result<String, ToolError> {
    args.get_str(key)
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::User(format!("Chybí povinný argument '{}'.", key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_root() -> PathBuf {
        std::env::temp_dir().join(format!("pm-mcp-test-{}", pm_core::util::new_id("t")))
    }

    fn call(root: &Path, raw: &str) -> Json {
        Json::parse(&handle_message(raw, root).expect("expected a response")).unwrap()
    }

    fn tool_text(resp: &Json) -> String {
        resp.get("result")
            .unwrap()
            .get("content")
            .unwrap()
            .as_arr()
            .unwrap()[0]
            .get_str("text")
            .unwrap()
            .to_string()
    }

    #[test]
    fn initialize_and_list() {
        let root = lib_root();
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
        );
        assert_eq!(
            r.get("result").unwrap().get("serverInfo").unwrap().get_str("name").unwrap(),
            "prompt-manager"
        );
        // notification -> no reply
        assert!(handle_message(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &root
        )
        .is_none());
        let r = call(&root, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = r
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_arr()
            .unwrap();
        assert!(tools.len() >= 10);
        assert!(tools
            .iter()
            .any(|t| t.get_str("name") == Some("render_prompt")));
    }

    #[test]
    fn tool_flow_create_render() {
        let root = lib_root();
        // create recipe
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_recipe","arguments":{"name":"R","values":{"cesta":"C:\\X"}}}}"#,
        );
        let recipe = Json::parse(&tool_text(&r)).unwrap();
        let rid = recipe.get_str("id").unwrap().to_string();

        // create prompt
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_prompt","arguments":{"title":"P","content":"Cesta: {{cesta}} a {{jina}}"}}}"#,
        );
        let prompt = Json::parse(&tool_text(&r)).unwrap();
        let _pid = prompt.get_str("id").unwrap();

        // render by title + recipe name
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render_prompt","arguments":{"id":"P","recipe":"R"}}}"#,
        );
        let out = Json::parse(&tool_text(&r)).unwrap();
        assert_eq!(out.get_str("text").unwrap(), "Cesta: C:\\X a {{jina}}");
        assert_eq!(out.get("missing").unwrap().as_arr().unwrap().len(), 1);
        let _ = rid;
    }

    #[test]
    fn tool_errors_are_soft() {
        let root = lib_root();
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_prompt","arguments":{"id":"neexistuje"}}}"#,
        );
        assert_eq!(
            r.get("result").unwrap().get("isError").unwrap().as_bool(),
            Some(true)
        );
        // unknown method -> protocol error
        let r = call(&root, r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#);
        assert!(r.get("error").is_some());
        // parse error
        let r = Json::parse(&handle_message("not json", &root).unwrap()).unwrap();
        assert!(r.get("error").is_some());
    }

    #[test]
    fn list_variables_tool() {
        let root = lib_root();
        call(
            &root,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_recipe","arguments":{"name":"A","values":{"x":"1","y":"2"}}}}"#,
        );
        let r = call(
            &root,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_variables","arguments":{}}}"#,
        );
        let vars = Json::parse(&tool_text(&r)).unwrap();
        assert_eq!(vars.as_arr().unwrap().len(), 2);
    }
}
