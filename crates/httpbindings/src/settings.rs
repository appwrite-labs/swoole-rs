//! The coroutine HTTP server's `start` settings, in upstream's order.

/// One `start` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// `http_parse_cookie`.
    Cookie,
    /// `http_parse_post`.
    Post,
    /// `http_parse_files`.
    Files,
    /// `http_compression`.
    Compression,
    /// `http_compression_level` and its aliases.
    Level,
    /// `http_compression_min_length` and its alias.
    Minimum,
    /// `http_compression_types` and its alias.
    Types,
    /// The WebSocket settings, read from the table as a whole.
    Websocket,
    /// `upload_tmp_dir`.
    Upload,
}

/// Each setting, the keys that set it, and whether it needs compression.
const SETTINGS: [(Setting, &[&[u8]], bool); 9] = [
    (Setting::Cookie, &[b"http_parse_cookie"], false),
    (Setting::Post, &[b"http_parse_post"], false),
    (Setting::Files, &[b"http_parse_files"], false),
    (Setting::Compression, &[b"http_compression"], true),
    (
        Setting::Level,
        &[
            b"http_compression_level",
            b"compression_level",
            b"http_gzip_level",
        ],
        true,
    ),
    (
        Setting::Minimum,
        &[b"http_compression_min_length", b"compression_min_length"],
        true,
    ),
    (
        Setting::Types,
        &[b"http_compression_types", b"compression_types"],
        true,
    ),
    (Setting::Websocket, &[], false),
    (Setting::Upload, &[b"upload_tmp_dir"], false),
];

/// The settings `start` applies, in order, each with the keys it looks up in turn:
/// the first whose value isn't null sets it. [`Setting::Websocket`] has none and
/// always applies. Without compression its settings are left out.
pub fn settings(compression: bool) -> impl Iterator<Item = (Setting, &'static [&'static [u8]])> {
    SETTINGS
        .into_iter()
        .filter(move |&(_, _, needs)| compression || !needs)
        .map(|(setting, keys, _)| (setting, keys))
}

/// Zend conversion remains native; preserve the original unsigned-byte clamp.
pub fn swr_v1_httpbindings_server_level(level: i64) -> u32 {
    u8::try_from(level.clamp(0, 255)).map_or(0, u32::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // Ported from tests/modules/httpbindings/settings_gate.cc: the settings order, driven as
    // crates/phphttp's `start` drives it, against upstream's `start` settings body.
    #[rustfmt::skip]
    const KEYS: [&str; 12] = [
        "http_parse_cookie", "http_parse_post", "http_parse_files", "http_compression",
        "http_compression_level", "compression_level", "http_gzip_level",
        "http_compression_min_length", "compression_min_length",
        "http_compression_types", "compression_types", "upload_tmp_dir",
    ];
    // An entry's value: null, a scalar or an array; absent keys are not in the table.
    const NULL: u8 = 0;
    const SCALAR: u8 = 1;
    const ARRAY: u8 = 2;
    type Table = BTreeMap<&'static str, u8>;
    struct Stop;
    fn pass(ok: bool) -> Result<(), Stop> {
        if ok { Ok(()) } else { Err(Stop) }
    }
    /// The native side both drivers share: lookups and effects in order, the reentrant
    /// hooks (an effect rewriting the settings array), the refusals (non-array types, an
    /// uncreatable `upload_tmp_dir`) and an injected exit at trace position `fail`.
    struct Host {
        table: Table,
        trace: Vec<String>,
        /// (compression, `upload_tmp_dir` creatable, hook)
        case: (bool, bool, usize),
        fail: usize,
    }
    impl Host {
        fn event(&mut self, event: String) -> Result<(), Stop> {
            self.trace.push(event);
            pass(self.trace.len() - 1 != self.fail)
        }
        /// `php_swoole_array_get_value`: found when present and not null.
        fn lookup(&mut self, key: &'static str) -> Result<bool, Stop> {
            self.event(format!("lookup:{key}"))?;
            Ok(self.table.get(key).is_some_and(|v| *v != NULL))
        }
        fn effect(&mut self, kind: u32, key: &'static str) -> Result<(), Stop> {
            self.event(format!("effect:{kind}:{key}"))?;
            let types = self.table.contains_key(KEYS[9]);
            match (self.case.2, kind) {
                (1, 2..=5) => self.table.insert(KEYS[5], SCALAR),
                (2, 9) => self.table.insert(KEYS[11], SCALAR),
                (3, 8) if types => self.table.insert(KEYS[9], ARRAY),
                _ => None,
            };
            let refused = kind == 8 && self.table.get(key) != Some(&ARRAY);
            pass(!refused && (kind != 10 || self.case.1))
        }
    }
    fn pinned(host: &mut Host, compression: bool) -> Result<(), Stop> {
        let groups = [
            (0..1, 2),
            (1..2, 3),
            (2..3, 4),
            (3..4, 5),
            (4..7, 6),
            (7..9, 7),
            (9..11, 8),
        ];
        for (keys, kind) in groups.into_iter().take(if compression { 7 } else { 3 }) {
            for key in &KEYS[keys] {
                if host.lookup(key)? {
                    host.effect(kind, key)?;
                    break;
                }
            }
        }
        host.effect(9, "")?;
        if host.lookup(KEYS[11])? {
            host.effect(10, KEYS[11])?;
        }
        host.event("done".into())
    }
    fn cursor(host: &mut Host, compression: bool) -> Result<(), Stop> {
        for (setting, keys) in settings(compression) {
            // The pinned body's effect numbers.
            let kind = match setting {
                Setting::Cookie => 2,
                Setting::Post => 3,
                Setting::Files => 4,
                Setting::Compression => 5,
                Setting::Level => 6,
                Setting::Minimum => 7,
                Setting::Types => 8,
                Setting::Websocket => 9,
                Setting::Upload => 10,
            };
            if keys.is_empty() {
                host.effect(kind, "")?;
            }
            for key in keys {
                let key = KEYS.into_iter().find(|k| k.as_bytes() == *key).unwrap();
                if host.lookup(key)? {
                    host.effect(kind, key)?;
                    break;
                }
            }
        }
        host.event("done".into())
    }
    type Driver = fn(&mut Host, bool) -> Result<(), Stop>;
    fn run(driver: Driver, table: &Table, case: (bool, bool, usize), fail: usize) -> Vec<String> {
        let mut host = Host {
            table: table.clone(),
            trace: vec![],
            case,
            fail,
        };
        // A refusal or an exit ends the trace; the trace is the outcome.
        driver(&mut host, case.0).unwrap_or_default();
        host.trace
    }

    /// Every subset of the twelve keys, eight value/hook variations, with and without
    /// compression, and an exit at each trace position for a sample of them.
    #[test]
    #[cfg_attr(miri, ignore = "65,536 cases are too slow to interpret")]
    fn cursor_matches_the_pinned_start_body() {
        let mut exits = 0;
        for (compression, mask, variation) in
            (0..2 * 4096 * 8).map(|i| (i >= 4096 * 8, i / 8 % 4096, i % 8))
        {
            let value = |k| match k {
                9 | 10 => [NULL, SCALAR, ARRAY][variation % 3],
                _ => u8::from(variation != 0),
            };
            let table = (0..12)
                .filter(|k| mask & (1 << k) != 0)
                .map(|k| (KEYS[k], value(k)))
                .collect();
            let case = (compression, variation & 1 != 0, variation % 4);
            let expected = run(pinned, &table, case, usize::MAX);
            assert_eq!(
                run(cursor, &table, case, usize::MAX),
                expected,
                "mask={mask} variation={variation}"
            );
            if mask % 127 == 0 && variation % 3 == 0 {
                for fail in 0..expected.len() {
                    assert_eq!(
                        run(cursor, &table, case, fail),
                        run(pinned, &table, case, fail)
                    );
                    exits += 1;
                }
            }
        }
        assert!(exits > 0);
    }

    #[test]
    fn pinned_alias_and_refusal_order() {
        let trace = |entries: &[(&'static str, u8)], dir_ok| {
            run(
                cursor,
                &entries.iter().copied().collect(),
                (true, dir_ok, 0),
                usize::MAX,
            )
            .join(" ")
        };
        // A false value is found, so `compression_level` is never looked up.
        let alias = trace(&[(KEYS[4], SCALAR), (KEYS[5], SCALAR)], true);
        assert!(
            alias.contains("effect:6:http_compression_level")
                && !alias.contains("lookup:compression_level")
        );
        // A null value falls through to the next alias.
        assert!(
            trace(&[(KEYS[4], NULL), (KEYS[5], SCALAR)], true)
                .contains("effect:6:compression_level")
        );
        // Non-array types refuse before the WebSocket settings, without trying the alias.
        let types = trace(&[(KEYS[9], SCALAR), (KEYS[10], ARRAY)], true);
        assert!(
            types.ends_with("effect:8:http_compression_types")
                && !types.contains("lookup:compression_types")
        );
        // The WebSocket settings apply before upload_tmp_dir fails.
        let directory = trace(&[(KEYS[11], SCALAR)], false);
        assert!(directory.ends_with("effect:9: lookup:upload_tmp_dir effect:10:upload_tmp_dir"));
    }

    #[test]
    fn level_clamps_to_an_unsigned_byte() {
        let levels = [i64::MIN, -1, 0, 1, 255, 256, i64::MAX];
        assert_eq!(
            levels.map(swr_v1_httpbindings_server_level),
            [0, 0, 0, 1, 255, 255, 255]
        );
    }
}
