//! The static handler's configuration: three string collections and the rewrite
//! rules, which the server's state owns.
use crate::ServerState;
use std::sync::{Arc, Mutex, MutexGuard};
use swoole_rs_http::{HttpStaticContext, RewriteRules};

/// One of the static handler's string collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    /// `locations`.
    Locations,
    /// `http_index_files`.
    IndexFiles,
    /// `http_compression_types`.
    CompressionTypes,
}

/// An operation on a [`Collection`], with what it returns from [`config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op<'a> {
    /// The number of values.
    Count,
    /// 1 when the collection is set, else 0.
    Enabled,
    /// Set the collection, keeping its values.
    Enable,
    /// Replace it with an unset, empty one.
    Reset,
    /// Set it and add the value unless present: the value's index when it was
    /// present, else the count after the add.
    Insert(&'a [u8]),
    /// The value's index, or the count when it is absent.
    Find(&'a [u8]),
    /// Set it and add the value even if present, as a public vector's appends do:
    /// as [`Op::Insert`].
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only tests assign a collection directly, as upstream's do"
        )
    )]
    Append(&'a [u8]),
}

#[derive(Default)]
struct Strings {
    /// Copy-on-write, so a reader takes them without holding the lock.
    values: Arc<Vec<Arc<[u8]>>>,
    enabled: bool,
}

#[derive(Default)]
pub(crate) struct StaticConfig {
    locations: Strings,
    index_files: Strings,
    compression_types: Strings,
    rewrite_rules: RewriteRules,
}

impl StaticConfig {
    fn strings(&mut self, collection: Collection) -> &mut Strings {
        match collection {
            Collection::Locations => &mut self.locations,
            Collection::IndexFiles => &mut self.index_files,
            Collection::CompressionTypes => &mut self.compression_types,
        }
    }
}

fn owner(state: &ServerState) -> MutexGuard<'_, StaticConfig> {
    state
        .static_config
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `collection`: whether it is set, and its values, read after the lock is released.
pub(crate) fn strings<R>(
    state: &ServerState,
    collection: Collection,
    f: impl FnOnce(bool, &[Arc<[u8]>]) -> R,
) -> R {
    let (enabled, values) = {
        let mut owner = owner(state);
        let strings = owner.strings(collection);
        (strings.enabled, Arc::clone(&strings.values))
    };
    f(enabled, &values)
}

/// `operation` on `collection`. The named index setter does its original duplicate
/// lookup before appending.
pub(crate) fn config(state: &ServerState, collection: Collection, operation: Op<'_>) -> usize {
    let mut owner = owner(state);
    let strings = owner.strings(collection);
    let find = |strings: &Strings, bytes: &[u8]| {
        let found = strings.values.iter().position(|value| **value == *bytes);
        found.ok_or(strings.values.len())
    };
    match operation {
        Op::Count => strings.values.len(),
        Op::Enabled => usize::from(strings.enabled),
        Op::Enable => {
            strings.enabled = true;
            0
        }
        Op::Reset => {
            *strings = Strings::default();
            0
        }
        Op::Find(bytes) => find(strings, bytes).unwrap_or_else(|count| count),
        Op::Insert(bytes) | Op::Append(bytes) => {
            let found = find(strings, bytes);
            strings.enabled = true;
            if found.is_err() || matches!(operation, Op::Append(_)) {
                Arc::make_mut(&mut strings.values).push(Arc::from(bytes));
            }
            found.unwrap_or(strings.values.len())
        }
    }
}

/// `url_rewrite_rules`: add one rule (upstream's `Server::add_rewrite_rule`).
pub(crate) fn add_rewrite_rule(state: &ServerState, pattern: &[u8], replacement: &[u8]) {
    owner(state).rewrite_rules.add(pattern, replacement);
}
/// Upstream's `Server::apply_rewrite_rules` over the server's rules.
pub(crate) fn apply_rewrite_rules(state: &ServerState, context: &mut HttpStaticContext) -> bool {
    context.rewrite(&owner(state).rewrite_rules)
}

#[cfg(test)]
mod tests {
    use super::Collection::{CompressionTypes, IndexFiles, Locations};
    use super::*;

    #[test]
    fn optional_collections_distinguish_absent_empty_and_ordered_unique_values() {
        let state = ServerState::new();
        let config = |collection, op| config(&state, collection, op);
        let text = |collection, index: usize| {
            strings(&state, collection, |_, values| values.get(index).cloned())
        };
        assert_eq!(config(Locations, Op::Enabled), 0);
        config(Locations, Op::Enable);
        assert_eq!(config(Locations, Op::Enabled), 1);
        assert_eq!(config(Locations, Op::Count), 0);
        config(IndexFiles, Op::Insert(b"index.php"));
        config(IndexFiles, Op::Insert(b"index.html"));
        config(IndexFiles, Op::Insert(b"index.php"));
        assert_eq!(config(IndexFiles, Op::Count), 2);
        assert_eq!(config(IndexFiles, Op::Find(b"index.php")), 0);
        assert_eq!(config(IndexFiles, Op::Find(b"missing")), 2);
        assert_eq!(text(IndexFiles, 1).as_deref(), Some(&b"index.html"[..]));
        config(CompressionTypes, Op::Insert(b"text/html"));
        config(CompressionTypes, Op::Insert(b"Text/Html"));
        assert_eq!(
            config(CompressionTypes, Op::Count),
            2,
            "MIME configuration preserves upstream's case-sensitive byte keys"
        );
        assert_eq!(
            config(Locations, Op::Count),
            0,
            "each collection owns separate state"
        );
        config(IndexFiles, Op::Append(b"index.php"));
        assert_eq!(
            config(IndexFiles, Op::Count),
            3,
            "direct public vector assignment/appends preserve duplicates"
        );
        config(IndexFiles, Op::Reset);
        assert_eq!(config(IndexFiles, Op::Enabled), 0);
        assert_eq!(config(IndexFiles, Op::Count), 0);
        assert!(text(IndexFiles, 0).is_none());
    }

    #[test]
    fn a_writer_proceeds_while_a_reader_holds_the_snapshot() {
        let state = Arc::new(ServerState::new());
        let append = |state: &ServerState, bytes: &'static [u8]| {
            config(state, IndexFiles, Op::Append(bytes))
        };
        append(&state, b"index.php");
        let writer = strings(&state, IndexFiles, |enabled, values| {
            assert!(enabled);
            let (done, wait) = std::sync::mpsc::channel();
            let shared = Arc::clone(&state);
            let writer = std::thread::spawn(move || done.send(append(&shared, b"index.html")));
            assert_eq!(
                wait.recv_timeout(std::time::Duration::from_secs(10)),
                Ok(2),
                "the append doesn't wait for the reader"
            );
            assert_eq!(values.len(), 1, "the reader keeps its snapshot");
            assert_eq!(&*values[0], b"index.php");
            writer
        });
        assert!(writer.join().is_ok_and(|sent| sent.is_ok()));
        assert_eq!(append(&state, b"default.htm"), 3);
    }

    #[test]
    fn rewrite_rules_rewrite_the_context_url() {
        let state = ServerState::new();
        let mut context = HttpStaticContext::new(b"/view/post/3", 128).unwrap();
        assert!(!apply_rewrite_rules(&state, &mut context));
        add_rewrite_rule(&state, b"~^/view/post/(\\d+)$~", b"/static/$1.html");
        add_rewrite_rule(&state, b"/article/", b"/static/article/");
        assert!(apply_rewrite_rules(&state, &mut context));
        assert!(
            !apply_rewrite_rules(&state, &mut context),
            "/static/3.html matches neither"
        );
    }

    // Ported from tests/modules/http/server_config_owners.cc, less its bound owners,
    // which only native static contexts held.
    #[test]
    fn locations_insert_uniquely_and_index_files_append_duplicates() {
        let state = ServerState::new();
        let config = |collection, op| config(&state, collection, op);
        // Locations: absent until the first insert, which is unique.
        assert_eq!(config(Locations, Op::Enabled), 0);
        config(Locations, Op::Insert(b"/"));
        config(Locations, Op::Insert(b"/"));
        assert_eq!(config(Locations, Op::Enabled), 1);
        assert_eq!(config(Locations, Op::Count), 1);
        // Index files: assignment replaces, enables and appends with duplicates.
        config(IndexFiles, Op::Reset);
        config(IndexFiles, Op::Enable);
        for name in [&b"a"[..], b"a", b"b"] {
            config(IndexFiles, Op::Append(name));
        }
        assert_eq!(config(IndexFiles, Op::Count), 3);
        let all = strings(&state, IndexFiles, |_, values| values.to_vec());
        assert_eq!(all, [&b"a"[..], b"a", b"b"].map(Arc::from));
        config(IndexFiles, Op::Append(b"b"));
        assert_eq!(config(IndexFiles, Op::Count), 4);
        assert!(config(IndexFiles, Op::Find(b"b")) < config(IndexFiles, Op::Count));
        // Compression types: a replacement forgets the earlier values.
        config(CompressionTypes, Op::Insert(b"text/html"));
        config(CompressionTypes, Op::Reset);
        config(CompressionTypes, Op::Append(b"text/plain"));
        assert_eq!(config(CompressionTypes, Op::Find(b"text/html")), 1);
        assert_eq!(config(CompressionTypes, Op::Find(b"text/plain")), 0);
    }
}
