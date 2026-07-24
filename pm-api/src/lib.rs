//! pm-api – local REST API for Prompt Manager.
//!
//! A small hand-rolled HTTP/1.1 server (std only) bound to 127.0.0.1.
//! Authentication: `Authorization: Bearer <key>`.
//!
//! Endpoints (all JSON):
//!   GET    /api/v1/prompts
//!   POST   /api/v1/prompts                 {title, content}
//!   GET    /api/v1/prompts/{id}
//!   PUT    /api/v1/prompts/{id}            {title?, content?, recipes?, selected_recipe?}
//!   DELETE /api/v1/prompts/{id}
//!   PUT    /api/v1/prompts/order           {prompts: [id, ...]}
//!   POST   /api/v1/prompts/{id}/render     {recipe_id?, overrides?}
//!   GET    /api/v1/recipes
//!   POST   /api/v1/recipes                 {name, values?}
//!   GET    /api/v1/recipes/{id}
//!   PUT    /api/v1/recipes/{id}            {name?, values?}
//!   DELETE /api/v1/recipes/{id}
//!   GET    /api/v1/variables

use pm_core::json::Json;
use pm_core::storage::StoreError;
use pm_core::{Library, Prompt, Recipe};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

#[derive(Clone)]
pub struct ApiConfig {
    pub library_root: PathBuf,
    pub api_key: String,
    pub port: u16,
}

pub struct ApiServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl ApiServer {
    /// Binds 127.0.0.1:port (port 0 = ephemeral) and serves in background
    /// threads until `stop()` is called or the process exits.
    pub fn start(cfg: ApiConfig) -> std::io::Result<ApiServer> {
        // one-off reconciliation of variables.json with the recipes; writes
        // nothing when the library is already consistent
        if let Ok(lib) = Library::open(&cfg.library_root) {
            if let Err(e) = lib.migrate() {
                eprintln!("library migration failed: {}", e);
            }
        }
        let listener = TcpListener::bind(("127.0.0.1", cfg.port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let cfg = Arc::new(cfg);
        thread::Builder::new()
            .name("pm-api-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(stream) = conn {
                        let cfg = cfg.clone();
                        let _ = thread::Builder::new()
                            .name("pm-api-conn".into())
                            .spawn(move || {
                                let _ = handle_connection(stream, &cfg);
                            });
                    }
                }
            })?;
        Ok(ApiServer { port, stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // wake the accept loop
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Result<Json, Resp> {
        if self.body.is_empty() {
            return Ok(Json::Obj(BTreeMap::new()));
        }
        let text = std::str::from_utf8(&self.body)
            .map_err(|_| Resp::error(400, "body is not valid UTF-8"))?;
        Json::parse(text).map_err(|e| Resp::error(400, &format!("invalid JSON: {}", e)))
    }

    pub fn test(method: &str, path: &str, body: &str) -> Request {
        Request {
            method: method.to_string(),
            path: path.to_string(),
            headers: vec![],
            body: body.as_bytes().to_vec(),
        }
    }
    pub fn with_auth(mut self, key: &str) -> Request {
        self.headers
            .push(("Authorization".to_string(), format!("Bearer {}", key)));
        self
    }
}

pub struct Resp {
    pub status: u16,
    pub body: Json,
}

impl Resp {
    fn ok(body: Json) -> Resp {
        Resp { status: 200, body }
    }
    fn error(status: u16, msg: &str) -> Resp {
        let mut o = BTreeMap::new();
        o.insert("error".to_string(), Json::str(msg));
        Resp {
            status,
            body: Json::Obj(o),
        }
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        _ => "Internal Server Error",
    }
}

fn handle_connection(stream: TcpStream, cfg: &ApiConfig) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let req = match read_request(&mut reader) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()), // connection closed
            Err(_) => return Ok(()),
        };
        let keep_alive = req
            .header("connection")
            .map(|c| !c.eq_ignore_ascii_case("close"))
            .unwrap_or(true);
        let resp = route(&req, cfg);
        write_response(&stream, &req, resp, keep_alive)?;
        if !keep_alive {
            return Ok(());
        }
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let path = parts.next().unwrap_or("/").to_string();
    if method.is_empty() {
        return Ok(None);
    }
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some(idx) = h.find(':') {
            headers.push((h[..idx].trim().to_string(), h[idx + 1..].trim().to_string()));
        }
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if len > 10 * 1024 * 1024 {
        return Ok(None); // refuse oversized bodies
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

fn write_response(
    mut stream: &TcpStream,
    req: &Request,
    resp: Resp,
    keep_alive: bool,
) -> std::io::Result<()> {
    let body = if req.method == "HEAD" {
        String::new()
    } else {
        resp.body.pretty()
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nConnection: {}\r\n\r\n",
        resp.status,
        status_text(resp.status),
        body.as_bytes().len(),
        if keep_alive { "keep-alive" } else { "close" }
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// Routes a request. Public for direct unit testing.
pub fn route(req: &Request, cfg: &ApiConfig) -> Resp {
    if req.method == "OPTIONS" {
        return Resp {
            status: 204,
            body: Json::Obj(BTreeMap::new()),
        };
    }
    // auth
    let authorized = req
        .header("authorization")
        .map(|v| {
            v.strip_prefix("Bearer ")
                .map(|k| constant_time_eq(k.trim(), &cfg.api_key))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if !authorized {
        return Resp::error(401, "missing or invalid API key");
    }

    let lib = match Library::open(&cfg.library_root) {
        Ok(l) => l,
        Err(e) => return store_err(e),
    };

    let path = req.path.split('?').next().unwrap_or("");
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    // expected: ["api", "v1", resource, ...]
    if segs.len() < 3 || segs[0] != "api" || segs[1] != "v1" {
        return Resp::error(404, "unknown path");
    }
    let m = req.method.as_str();
    match (m, &segs[2..]) {
        ("GET", ["prompts"]) => list_prompts(&lib),
        ("POST", ["prompts"]) => create_prompt(&lib, req),
        ("PUT", ["prompts", "order"]) => set_order(&lib, req),
        ("GET", ["prompts", id]) => get_prompt(&lib, id),
        ("PUT", ["prompts", id]) => update_prompt(&lib, id, req),
        ("DELETE", ["prompts", id]) => delete_prompt(&lib, id),
        ("POST", ["prompts", id, "render"]) => render_prompt(&lib, id, req),
        ("GET", ["recipes"]) => list_recipes(&lib),
        ("POST", ["recipes"]) => create_recipe(&lib, req),
        ("GET", ["recipes", id]) => get_recipe(&lib, id),
        ("PUT", ["recipes", id]) => update_recipe(&lib, id, req),
        ("DELETE", ["recipes", id]) => delete_recipe(&lib, id),
        ("GET", ["variables"]) => list_variables(&lib),
        ("GET", _) | ("POST", _) | ("PUT", _) | ("DELETE", _) => Resp::error(404, "unknown path"),
        _ => Resp::error(405, "method not allowed"),
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn store_err(e: StoreError) -> Resp {
    match e {
        StoreError::NotFound(id) => Resp::error(404, &format!("not found: {}", id)),
        StoreError::Invalid(m) => Resp::error(400, &m),
        StoreError::Parse(f, m) => Resp::error(500, &format!("cannot parse {}: {}", f, m)),
        StoreError::Io(e) => Resp::error(500, &format!("io error: {}", e)),
    }
}

// ---------- handlers ----------

fn list_prompts(lib: &Library) -> Resp {
    match lib.list_prompts() {
        Ok(ps) => Resp::ok(Json::Arr(ps.iter().map(|p| p.to_json()).collect())),
        Err(e) => store_err(e),
    }
}

fn get_prompt(lib: &Library, id: &str) -> Resp {
    match lib.get_prompt(id) {
        Ok(p) => Resp::ok(p.to_json()),
        Err(e) => store_err(e),
    }
}

fn create_prompt(lib: &Library, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let title = body.get_str("title").unwrap_or("").trim().to_string();
    if title.is_empty() {
        return Resp::error(400, "title is required");
    }
    let content = body.get_str("content").unwrap_or("").to_string();
    match lib.create_prompt(&title, &content) {
        Ok(mut p) => {
            // optional immediate assignment
            let mut changed = false;
            if let Some(rs) = body.get("recipes").and_then(|a| a.as_arr()) {
                p.recipes = rs
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
                changed = true;
            }
            if let Some(sel) = body.get_str("selected_recipe") {
                p.selected_recipe = Some(sel.to_string());
                changed = true;
            }
            if changed {
                match lib.save_prompt(&p) {
                    Ok(p2) => Resp {
                        status: 201,
                        body: p2.to_json(),
                    },
                    Err(e) => store_err(e),
                }
            } else {
                Resp {
                    status: 201,
                    body: p.to_json(),
                }
            }
        }
        Err(e) => store_err(e),
    }
}

fn update_prompt(lib: &Library, id: &str, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut p: Prompt = match lib.get_prompt(id) {
        Ok(p) => p,
        Err(e) => return store_err(e),
    };
    if let Some(t) = body.get_str("title") {
        if t.trim().is_empty() {
            return Resp::error(400, "title must not be empty");
        }
        p.title = t.to_string();
    }
    if let Some(c) = body.get_str("content") {
        p.content = c.to_string();
    }
    if let Some(rs) = body.get("recipes").and_then(|a| a.as_arr()) {
        p.recipes = rs
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(v) = body.get("selected_recipe") {
        p.selected_recipe = v.as_str().map(|s| s.to_string());
    }
    match lib.save_prompt(&p) {
        Ok(p) => Resp::ok(p.to_json()),
        Err(e) => store_err(e),
    }
}

fn delete_prompt(lib: &Library, id: &str) -> Resp {
    match lib.delete_prompt(id) {
        Ok(()) => Resp::ok(Json::Obj(BTreeMap::new())),
        Err(e) => store_err(e),
    }
}

fn set_order(lib: &Library, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ids: Vec<String> = body
        .get("prompts")
        .and_then(|a| a.as_arr())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    match lib.reorder_prompts(&ids) {
        Ok(order) => {
            let mut o = BTreeMap::new();
            o.insert(
                "prompts".to_string(),
                Json::Arr(order.iter().map(|i| Json::str(i)).collect()),
            );
            Resp::ok(Json::Obj(o))
        }
        Err(e) => store_err(e),
    }
}

fn render_prompt(lib: &Library, id: &str, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let recipe_id = body.get("recipe_id").and_then(|v| v.as_str());
    let mut overrides = BTreeMap::new();
    if let Some(o) = body.get("overrides").and_then(|v| v.as_obj()) {
        for (k, v) in o {
            if let Some(s) = v.as_str() {
                overrides.insert(k.clone(), s.to_string());
            }
        }
    }
    match lib.render_prompt(id, recipe_id, &overrides) {
        Ok(r) => {
            let mut o = BTreeMap::new();
            o.insert("text".to_string(), Json::str(&r.text));
            o.insert(
                "missing".to_string(),
                Json::Arr(r.missing.iter().map(|m| Json::str(m)).collect()),
            );
            Resp::ok(Json::Obj(o))
        }
        Err(e) => store_err(e),
    }
}

fn list_recipes(lib: &Library) -> Resp {
    match lib.list_recipes() {
        Ok(rs) => Resp::ok(Json::Arr(rs.iter().map(|r| r.to_json()).collect())),
        Err(e) => store_err(e),
    }
}

fn get_recipe(lib: &Library, id: &str) -> Resp {
    match lib.get_recipe(id) {
        Ok(r) => Resp::ok(r.to_json()),
        Err(e) => store_err(e),
    }
}

fn create_recipe(lib: &Library, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = body.get_str("name").unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Resp::error(400, "name is required");
    }
    match lib.create_recipe(&name) {
        Ok(mut r) => {
            if let Some(vals) = body.get("values").and_then(|v| v.as_obj()) {
                r.values = vals
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
                return match lib.save_recipe(&r) {
                    Ok(r2) => Resp {
                        status: 201,
                        body: r2.to_json(),
                    },
                    Err(e) => {
                        // roll back the empty recipe file to avoid orphans
                        let _ = lib.delete_recipe(&r.id);
                        store_err(e)
                    }
                };
            }
            Resp {
                status: 201,
                body: r.to_json(),
            }
        }
        Err(e) => store_err(e),
    }
}

fn update_recipe(lib: &Library, id: &str, req: &Request) -> Resp {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut r: Recipe = match lib.get_recipe(id) {
        Ok(r) => r,
        Err(e) => return store_err(e),
    };
    if let Some(n) = body.get_str("name") {
        if n.trim().is_empty() {
            return Resp::error(400, "name must not be empty");
        }
        r.name = n.to_string();
    }
    if let Some(vals) = body.get("values").and_then(|v| v.as_obj()) {
        r.values = vals
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();
    }
    match lib.save_recipe(&r) {
        Ok(r) => Resp::ok(r.to_json()),
        Err(e) => store_err(e),
    }
}

fn delete_recipe(lib: &Library, id: &str) -> Resp {
    match lib.delete_recipe(id) {
        Ok(affected) => {
            let mut o = BTreeMap::new();
            o.insert(
                "removed_from_prompts".to_string(),
                Json::Arr(affected.iter().map(|t| Json::str(t)).collect()),
            );
            Resp::ok(Json::Obj(o))
        }
        Err(e) => store_err(e),
    }
}

fn list_variables(lib: &Library) -> Resp {
    match lib.variables() {
        Ok(vars) => {
            let mut arr = Vec::new();
            for (name, uses) in vars {
                let mut o = BTreeMap::new();
                o.insert("name".to_string(), Json::str(&name));
                o.insert(
                    "recipes".to_string(),
                    Json::Arr(
                        uses.iter()
                            .map(|(rid, rname, val)| {
                                let mut u = BTreeMap::new();
                                u.insert("recipe_id".to_string(), Json::str(rid));
                                u.insert("recipe_name".to_string(), Json::str(rname));
                                u.insert("value".to_string(), Json::str(val));
                                Json::Obj(u)
                            })
                            .collect(),
                    ),
                );
                arr.push(Json::Obj(o));
            }
            Resp::ok(Json::Arr(arr))
        }
        Err(e) => store_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn cfg() -> ApiConfig {
        let dir = std::env::temp_dir().join(format!(
            "pm-api-test-{}",
            pm_core::util::new_id("t")
        ));
        ApiConfig {
            library_root: dir,
            api_key: "test-key".into(),
            port: 0,
        }
    }

    fn call(cfg: &ApiConfig, method: &str, path: &str, body: &str) -> Resp {
        route(&Request::test(method, path, body).with_auth(&cfg.api_key), cfg)
    }

    #[test]
    fn auth_required() {
        let c = cfg();
        let r = route(&Request::test("GET", "/api/v1/prompts", ""), &c);
        assert_eq!(r.status, 401);
        let r = route(
            &Request::test("GET", "/api/v1/prompts", "").with_auth("wrong"),
            &c,
        );
        assert_eq!(r.status, 401);
        let r = call(&c, "GET", "/api/v1/prompts", "");
        assert_eq!(r.status, 200);
    }

    #[test]
    fn full_crud_flow() {
        let c = cfg();
        // create recipe
        let r = call(
            &c,
            "POST",
            "/api/v1/recipes",
            r#"{"name":"R1","values":{"cesta":"C:\\X","jazyk":"rust"}}"#,
        );
        assert_eq!(r.status, 201);
        let rid = r.body.get_str("id").unwrap().to_string();

        // create prompt
        let r = call(
            &c,
            "POST",
            "/api/v1/prompts",
            r#"{"title":"P1","content":"Ahoj {{cesta}} {{chybi}}"}"#,
        );
        assert_eq!(r.status, 201);
        let pid = r.body.get_str("id").unwrap().to_string();
        assert_eq!(
            r.body.get("placeholders").unwrap().as_arr().unwrap().len(),
            2
        );

        // assign recipe
        let r = call(
            &c,
            "PUT",
            &format!("/api/v1/prompts/{}", pid),
            &format!(r#"{{"recipes":["{}"],"selected_recipe":"{}"}}"#, rid, rid),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.body.get_str("selected_recipe").unwrap(), rid);

        // render with recipe
        let r = call(
            &c,
            "POST",
            &format!("/api/v1/prompts/{}/render", pid),
            &format!(r#"{{"recipe_id":"{}"}}"#, rid),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.body.get_str("text").unwrap(), "Ahoj C:\\X {{chybi}}");
        assert_eq!(r.body.get("missing").unwrap().as_arr().unwrap().len(), 1);

        // render with override
        let r = call(
            &c,
            "POST",
            &format!("/api/v1/prompts/{}/render", pid),
            &format!(
                r#"{{"recipe_id":"{}","overrides":{{"chybi":"DOPLNĚNO"}}}}"#,
                rid
            ),
        );
        assert_eq!(r.body.get_str("text").unwrap(), "Ahoj C:\\X DOPLNĚNO");

        // variables
        let r = call(&c, "GET", "/api/v1/variables", "");
        assert_eq!(r.body.as_arr().unwrap().len(), 2);

        // order
        let r = call(
            &c,
            "POST",
            "/api/v1/prompts",
            r#"{"title":"P2","content":"x"}"#,
        );
        let pid2 = r.body.get_str("id").unwrap().to_string();
        let r = call(
            &c,
            "PUT",
            "/api/v1/prompts/order",
            &format!(r#"{{"prompts":["{}","{}"]}}"#, pid2, pid),
        );
        assert_eq!(r.status, 200);
        let r = call(&c, "GET", "/api/v1/prompts", "");
        let arr = r.body.as_arr().unwrap();
        assert_eq!(arr[0].get_str("id").unwrap(), pid2);

        // delete recipe cleans references
        let r = call(&c, "DELETE", &format!("/api/v1/recipes/{}", rid), "");
        assert_eq!(r.status, 200);
        let r = call(&c, "GET", &format!("/api/v1/prompts/{}", pid), "");
        assert!(r.body.get("recipes").unwrap().as_arr().unwrap().is_empty());

        // delete prompt
        let r = call(&c, "DELETE", &format!("/api/v1/prompts/{}", pid), "");
        assert_eq!(r.status, 200);
        let r = call(&c, "GET", &format!("/api/v1/prompts/{}", pid), "");
        assert_eq!(r.status, 404);
    }

    #[test]
    fn validation_errors() {
        let c = cfg();
        assert_eq!(call(&c, "POST", "/api/v1/prompts", r#"{}"#).status, 400);
        assert_eq!(call(&c, "POST", "/api/v1/prompts", "not json").status, 400);
        assert_eq!(call(&c, "GET", "/api/v1/nope", "").status, 404);
        assert_eq!(
            call(&c, "POST", "/api/v1/recipes", r#"{"name":""}"#).status,
            400
        );
        assert_eq!(
            call(
                &c,
                "POST",
                "/api/v1/recipes",
                r#"{"name":"X","values":{"ŠPATNĚ":"1"}}"#
            )
            .status,
            400
        );
    }

    #[test]
    fn live_server_over_tcp() {
        let c = cfg();
        // seed one prompt
        call(&c, "POST", "/api/v1/prompts", r#"{"title":"T","content":"c"}"#);
        let server = ApiServer::start(c.clone()).unwrap();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let req = format!(
            "GET /api/v1/prompts HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            c.api_key
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 200"), "got: {}", &buf[..40]);
        assert!(buf.contains("\"title\": \"T\""), "body: {}", buf);
        // unauthorized over TCP
        let mut s = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.write_all(b"GET /api/v1/prompts HTTP/1.1\r\nHost: l\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 401"));
        server.stop();
    }
}
