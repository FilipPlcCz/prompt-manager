//! Minimal, dependency-free JSON parser/serializer.
//! Supports the full JSON grammar (objects, arrays, strings with escapes
//! incl. \uXXXX surrogate pairs, numbers, booleans, null).

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.as_obj().and_then(|o| o.get(key))
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }

    pub fn parse(input: &str) -> Result<Json, String> {
        let mut p = Parser {
            b: input.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("trailing characters at byte {}", p.i));
        }
        Ok(v)
    }

    /// Compact serialization.
    pub fn dump(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }

    /// Pretty serialization with 2-space indent.
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 9.0e15 {
                    let _ = fmt::Write::write_fmt(out, format_args!("{}", *n as i64));
                } else {
                    let _ = fmt::Write::write_fmt(out, format_args!("{}", n));
                }
            }
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(a) => {
                if a.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (k, v) in a.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    newline_indent(out, indent, depth + 1);
                    v.write(out, indent, depth + 1);
                }
                newline_indent(out, indent, depth);
                out.push(']');
            }
            Json::Obj(o) => {
                if o.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (k, (key, v)) in o.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    newline_indent(out, indent, depth + 1);
                    write_json_string(out, key);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                newline_indent(out, indent, depth);
                out.push('}');
            }
        }
    }
}

fn newline_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..n * depth {
            out.push(' ');
        }
    }
}

pub fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = fmt::Write::write_fmt(out, format_args!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!(
                "expected '{}' at byte {}, found {:?}",
                c as char,
                self.i,
                self.peek().map(|x| x as char)
            ))
        }
    }
    fn lit(&mut self, s: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            Ok(v)
        } else {
            Err(format!("invalid literal at byte {}", self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            other => Err(format!(
                "unexpected {:?} at byte {}",
                other.map(|x| x as char),
                self.i
            )),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.eat(b'{')?;
        let mut map = BTreeMap::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            let val = self.value()?;
            map.insert(key, val);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(map));
                }
                other => {
                    return Err(format!(
                        "expected ',' or '}}' at byte {}, found {:?}",
                        self.i,
                        other.map(|x| x as char)
                    ))
                }
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.eat(b'[')?;
        let mut arr = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(arr));
        }
        loop {
            self.ws();
            arr.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(arr));
                }
                other => {
                    return Err(format!(
                        "expected ',' or ']' at byte {}, found {:?}",
                        self.i,
                        other.map(|x| x as char)
                    ))
                }
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut s = String::new();
        loop {
            let c = *self
                .b
                .get(self.i)
                .ok_or_else(|| "unterminated string".to_string())?;
            self.i += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    let e = *self
                        .b
                        .get(self.i)
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{0008}'),
                        b'f' => s.push('\u{000C}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                // surrogate pair
                                if self.b.get(self.i) == Some(&b'\\')
                                    && self.b.get(self.i + 1) == Some(&b'u')
                                {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err("invalid low surrogate".into());
                                    }
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    return Err("lone high surrogate".into());
                                }
                            } else {
                                hi
                            };
                            s.push(
                                char::from_u32(cp)
                                    .ok_or_else(|| "invalid codepoint".to_string())?,
                            );
                        }
                        _ => return Err(format!("invalid escape '\\{}'", e as char)),
                    }
                }
                _ => {
                    // collect UTF-8 continuation bytes as-is
                    let start = self.i - 1;
                    let len = utf8_len(c);
                    let end = start + len;
                    if end > self.b.len() {
                        return Err("truncated UTF-8".into());
                    }
                    self.i = end;
                    s.push_str(
                        std::str::from_utf8(&self.b[start..end])
                            .map_err(|_| "invalid UTF-8".to_string())?,
                    );
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.b.len() {
            return Err("truncated \\u escape".into());
        }
        let hex = std::str::from_utf8(&self.b[self.i..self.i + 4])
            .map_err(|_| "invalid hex".to_string())?;
        self.i += 4;
        u32::from_str_radix(hex, 16).map_err(|_| "invalid hex".into())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while self
            .peek()
            .map(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
            .unwrap_or(false)
        {
            self.i += 1;
        }
        let txt = std::str::from_utf8(&self.b[start..self.i]).unwrap();
        txt.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number '{}'", txt))
    }
}

fn utf8_len(first: u8) -> usize {
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

    #[test]
    fn roundtrip_basic() {
        let src = r#"{"a":[1,2.5,-3],"b":"x\ny","c":true,"d":null,"e":{"nested":"čeština 🙂"}}"#;
        let v = Json::parse(src).unwrap();
        let dumped = v.dump();
        let v2 = Json::parse(&dumped).unwrap();
        assert_eq!(v, v2);
        assert_eq!(v.get("b").unwrap().as_str().unwrap(), "x\ny");
        assert_eq!(
            v.get("e").unwrap().get_str("nested").unwrap(),
            "čeština 🙂"
        );
    }

    #[test]
    fn unicode_escapes() {
        let v = Json::parse(r#""é😀""#).unwrap();
        assert_eq!(v.as_str().unwrap(), "é😀");
    }

    #[test]
    fn escapes_out() {
        let v = Json::Str("a\"b\\c\nd\tctrl:\u{1}".into());
        // exact form checked via roundtrip; control chars must escape
        assert_eq!(Json::parse(&v.dump()).unwrap(), v);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("{}x").is_err());
        assert!(Json::parse("'single'").is_err());
    }

    #[test]
    fn pretty_parses_back() {
        let v = Json::parse(r#"{"prompts":["a","b"],"n":3}"#).unwrap();
        assert_eq!(Json::parse(&v.pretty()).unwrap(), v);
    }
}
