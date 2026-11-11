//! Ordered coroutine HTTP startup settings; native code owns all Zend effects.
use crate::{Action, Text};

#[repr(C)]
#[derive(Default)]
pub struct SettingsCursor {
    pub entry: u32,
    pub alias: u32,
    pub stage: u32,
}
struct Setting {
    keys: &'static [&'static [u8]],
    effect: u32,
    compression: bool,
}
const SETTINGS: [Setting; 9] = [
    Setting {
        keys: &[b"http_parse_cookie"],
        effect: 2,
        compression: false,
    },
    Setting {
        keys: &[b"http_parse_post"],
        effect: 3,
        compression: false,
    },
    Setting {
        keys: &[b"http_parse_files"],
        effect: 4,
        compression: false,
    },
    Setting {
        keys: &[b"http_compression"],
        effect: 5,
        compression: true,
    },
    Setting {
        keys: &[
            b"http_compression_level",
            b"compression_level",
            b"http_gzip_level",
        ],
        effect: 6,
        compression: true,
    },
    Setting {
        keys: &[b"http_compression_min_length", b"compression_min_length"],
        effect: 7,
        compression: true,
    },
    Setting {
        keys: &[b"http_compression_types", b"compression_types"],
        effect: 8,
        compression: true,
    },
    Setting {
        keys: &[],
        effect: 9,
        compression: false,
    },
    Setting {
        keys: &[b"upload_tmp_dir"],
        effect: 10,
        compression: false,
    },
];
fn advance(cursor: &mut SettingsCursor) {
    cursor.entry += 1;
    cursor.alias = 0;
    cursor.stage = 0;
}

/// Emit one native lookup/effect. Native lookup treats null values as absent,
/// preserving original short-circuit aliases and fresh conversion/reentry order.
#[must_use]
pub fn swr_v1_httpbindings_settings(
    cursor: Option<&mut SettingsCursor>,
    compression: u32,
    found: u32,
    out: Option<&mut Action>,
) -> i32 {
    let (Some(cursor), Some(out)) = (cursor, out) else {
        return -1;
    };
    *out = Action::default();
    if cursor.stage > 1
        || cursor.entry > u32::try_from(SETTINGS.len()).unwrap_or(u32::MAX)
        || (cursor.stage == 0 && cursor.alias != 0)
    {
        return -1;
    }
    if let Some(setting) = SETTINGS.get(cursor.entry as usize) {
        if cursor.stage == 1 && cursor.alias as usize >= setting.keys.len() {
            return -1;
        }
    } else if cursor.stage != 0 {
        return -1;
    }
    loop {
        let Some(setting) = SETTINGS.get(cursor.entry as usize) else {
            out.kind = 11;
            return 0;
        };
        if setting.compression && compression == 0 {
            advance(cursor);
            continue;
        }
        if setting.keys.is_empty() {
            out.kind = setting.effect;
            advance(cursor);
            return 0;
        }
        if cursor.stage == 1 {
            if cursor.alias as usize >= setting.keys.len() {
                return -1;
            }
            if found != 0 {
                out.kind = setting.effect;
                advance(cursor);
                return 0;
            }
            cursor.alias += 1;
            cursor.stage = 0;
        }
        let Some(key) = setting.keys.get(cursor.alias as usize) else {
            advance(cursor);
            continue;
        };
        out.kind = 1;
        out.text = Text {
            data: key.as_ptr(),
            length: key.len(),
        };
        cursor.stage = 1;
        return 0;
    }
}

/// Zend conversion remains native; preserve the original unsigned-byte clamp.
pub fn swr_v1_httpbindings_server_level(level: i64) -> u32 {
    u8::try_from(level.clamp(0, 255)).map_or(0, u32::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // Ported from tests/modules/httpbindings/settings_gate.cc: the cursor, driven as
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
        let (mut cursor, mut found, mut key) = (SettingsCursor::default(), 0, "");
        loop {
            let mut action = Action::default();
            let compression = u32::from(compression);
            let status = swr_v1_httpbindings_settings(
                Some(&mut cursor),
                compression,
                found,
                Some(&mut action),
            );
            assert_eq!(status, 0);
            match action.kind {
                1 => {
                    let text =
                        unsafe { std::slice::from_raw_parts(action.text.data, action.text.length) };
                    key = KEYS.into_iter().find(|k| k.as_bytes() == text).unwrap();
                    found = u32::from(host.lookup(key)?);
                }
                9 => host.effect(9, "")?,
                11 => return host.event("done".into()),
                kind => host.effect(kind, key)?,
            }
        }
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
        driver(&mut host, case.0).ok();
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

    #[test]
    fn malformed_cursors_do_not_request_a_native_effect() {
        for mut cursor in [
            SettingsCursor {
                entry: u32::MAX,
                alias: 0,
                stage: 0,
            },
            SettingsCursor {
                entry: 0,
                alias: u32::MAX,
                stage: 1,
            },
            SettingsCursor {
                entry: 0,
                alias: 1,
                stage: 0,
            },
            SettingsCursor {
                entry: 7,
                alias: 0,
                stage: 1,
            },
            SettingsCursor {
                entry: 9,
                alias: 0,
                stage: 1,
            },
        ] {
            let mut action = Action::default();
            assert_eq!(
                swr_v1_httpbindings_settings(Some(&mut cursor), 1, 0, Some(&mut action)),
                -1
            );
            assert_eq!(action.kind, 0);
        }
    }
}
