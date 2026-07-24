//! IDs, timestamps and slugs without external dependencies.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique, sortable id: `<prefix>-<unix_nanos_hex><counter_hex><mix_hex>`.
/// Not cryptographic – used only as a stable identifier.
pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mix = {
        // address entropy (ASLR) xor pid
        let b = Box::new(0u8);
        (Box::as_ref(&b) as *const u8 as u64) ^ (std::process::id() as u64) << 17
    };
    format!("{}-{:x}{:02x}{:04x}", prefix, nanos, c & 0xff, mix & 0xffff)
}

/// Random-ish hex token for the API key. Mixes several entropy sources and
/// stretches them with FNV-1a. Good enough for a loopback-only bearer token;
/// regenerate any time from Settings.
pub fn new_token() -> String {
    fn mix(h: &mut u64, v: u64) {
        for b in v.to_le_bytes() {
            *h ^= b as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
    let mut h: u64 = 0xcbf29ce484222325;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    mix(&mut h, nanos as u64);
    mix(&mut h, (nanos >> 64) as u64);
    mix(&mut h, std::process::id() as u64);
    mix(&mut h, COUNTER.fetch_add(1, Ordering::Relaxed) ^ 0x5851f42d4c957f2d);
    let b = Box::new(0u8);
    mix(&mut h, Box::as_ref(&b) as *const u8 as u64);
    let t = std::thread::current().id();
    mix(&mut h, &t as *const _ as u64);
    let mut out = String::new();
    for i in 0..4 {
        mix(&mut h, 0x9e3779b97f4a7c15 ^ i);
        out.push_str(&format!("{:016x}", h));
    }
    out
}

/// Current UTC time as ISO-8601 `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    iso_from_unix(secs as i64)
}

/// Civil-from-days algorithm (Howard Hinnant, public domain).
pub fn iso_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, mth, d, h, m, s
    )
}

/// File-name slug from a human title: lowercase ASCII, Czech diacritics folded,
/// everything else becomes `-`.
pub fn slugify(title: &str) -> String {
    const FROM: &str = "áäčďéěíĺľňóôöřŕšťúůüýžÁÄČĎÉĚÍĹĽŇÓÔÖŘŔŠŤÚŮÜÝŽ";
    const TO: &str = "aacdeeillnooorrstuuuyzAACDEEILLNOOORRSTUUUYZ";
    let fold: std::collections::HashMap<char, char> =
        FROM.chars().zip(TO.chars()).collect();
    let mut out = String::new();
    let mut last_dash = true;
    for c in title.chars() {
        let c = *fold.get(&c).unwrap_or(&c);
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique() {
        let a: Vec<String> = (0..100).map(|_| new_id("p")).collect();
        let set: std::collections::HashSet<_> = a.iter().collect();
        assert_eq!(set.len(), 100);
        assert!(a[0].starts_with("p-"));
    }

    #[test]
    fn tokens_long_and_distinct() {
        let t1 = new_token();
        let t2 = new_token();
        assert_eq!(t1.len(), 64);
        assert_ne!(t1, t2);
    }

    #[test]
    fn iso_known_values() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_from_unix(1_753_228_800), "2025-07-23T00:00:00Z");
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Code review – Rust"), "code-review-rust");
        assert_eq!(slugify("Žluťoučký kůň!!"), "zlutoucky-kun");
        assert_eq!(slugify("---"), "untitled");
        assert_eq!(slugify("Překlad EN → CZ (technický)"), "preklad-en-cz-technicky");
    }
}
