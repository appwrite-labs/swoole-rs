//! The cases of the retired `tests/modules/http/context_gate.py` and
//! `conditional_gate.py`: [`StaticHandler`] over a scripted filesystem, MIME
//! table and random strings, with libc's calendar recorded, against the pinned
//! `StaticHandler` over the same script (`swoole_rs_oracle::http_static_context`),
//! comparing every answer, every effect in order, and errno.
use super::{Effects, StaticHandler};
use crate::ServerConfig;
use crate::ops::OP_ST_SET_ROOT;
use crate::state::{ServerState, native_fact};
use core::cell::RefCell;
use core::ffi::{CStr, c_char, c_long, c_void};
use core::ptr;
use std::ffi::CString;
use swoole_rs_corebase::sys::set_errno;
use swoole_rs_http::{BOUNDARY_SIZE, Calendar, HttpStaticObservation, StaticEffects, Tm};
use swoole_rs_oracle::http_static_context as oracle;

#[test]
fn the_404_page_is_upstreams() {
    assert_eq!(
        super::page_404(),
        b"<html><body><style> body { padding: 0.5em; line-height: 2; } h1 { font-size: 1.5em; \
          padding-bottom: 0.3em; border-bottom: 1px solid #ccc; } ul { list-style-type: disc; } \
          footer { border-top: 1px solid #ccc; } a { color: #0969da; } </style><h1>HTTP 404 Not \
          Found</h1><footer><i>Powered by Swoole</i></footer></body></html>"
    );
}

/// What the scripted filesystem and MIME table answer (see [`oracle::Fs`]).
#[derive(Clone, Default)]
struct Script {
    file_size: usize,
    file_time: i64,
    directory: Vec<u8>,
    link: Vec<u8>,
    canonical: Vec<u8>,
    mime: Vec<u8>,
    names: Vec<Vec<u8>>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::default();
    static TRACE: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

fn note(parts: &[&[u8]]) {
    TRACE.with_borrow_mut(|trace| trace.push(parts.concat()));
}

fn c(bytes: &[u8]) -> CString {
    CString::new(bytes).unwrap()
}

/// Both sides answer from `script`, with empty traces.
fn script(script: Script) {
    let (directory, link, canonical, mime) = (
        c(&script.directory),
        c(&script.link),
        c(&script.canonical),
        c(&script.mime),
    );
    let names: Vec<CString> = script.names.iter().map(|name| c(name)).collect();
    let names: Vec<*const c_char> = names.iter().map(|name| name.as_ptr()).collect();
    let fs = oracle::Fs {
        file_size: script.file_size,
        file_time: script.file_time,
        directory: directory.as_ptr(),
        link: link.as_ptr(),
        canonical: canonical.as_ptr(),
        mime: mime.as_ptr(),
        names: names.as_ptr(),
        name_count: names.len(),
    };
    // SAFETY: the oracle copies every string.
    unsafe { oracle::swr_oracle_static_script(&raw const fs) };
    SCRIPT.set(script);
    TRACE.take();
}

/// The candidate's traced effects since the last call.
fn trace() -> Vec<Vec<u8>> {
    TRACE.take()
}

unsafe extern "C" fn push(out: *mut c_void, line: *const c_char, length: usize) {
    // SAFETY: `out` is the caller's `Vec`, `line` `length` readable bytes.
    let lines = unsafe { &mut *out.cast::<Vec<Vec<u8>>>() };
    // SAFETY: as above.
    lines.push(unsafe { core::slice::from_raw_parts(line.cast::<u8>(), length) }.to_vec());
}

/// The lines `call` sends to its [`oracle::Line`].
fn lines(call: impl FnOnce(oracle::Line, *mut c_void)) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    call(push, (&raw mut lines).cast());
    lines
}

/// The upstream side's traced effects, and its calendar events, since the last call.
fn upstream_trace() -> Vec<Vec<u8>> {
    // SAFETY: a live collector.
    lines(|line, out| unsafe { oracle::swr_oracle_static_trace(line, out) })
}

fn calendar_events() -> Vec<Vec<u8>> {
    // SAFETY: a live collector.
    lines(|line, out| unsafe { oracle::swr_oracle_calendar_events(line, out) })
}

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

fn os_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The path kinds (`crates/http/src/static_path.rs`).
const LINK: u32 = 8;
const DIRECTORY: u32 = 16;
const FILE: u32 = 32;

/// The candidate's effects: the script, traced as the oracle traces its own.
struct Scripted;

impl StaticEffects for Scripted {
    fn stat(&self, path: &[u8]) -> Option<HttpStaticObservation> {
        note(&[b"stat:", path]);
        Some(SCRIPT.with_borrow(|script| HttpStaticObservation {
            filesize: script.file_size,
            mtime: script.file_time,
            kind: if script.link == path {
                LINK
            } else if script.directory == path {
                DIRECTORY
            } else {
                FILE
            },
            ..HttpStaticObservation::default()
        }))
    }
    fn canonical(&self, path: &[u8]) -> Option<Vec<u8>> {
        note(&[b"canonical:", path]);
        let resolved = SCRIPT.with_borrow(|script| {
            if script.canonical.is_empty() {
                path.to_vec()
            } else {
                script.canonical.clone()
            }
        });
        // realpath's C string.
        Some(c_text(&resolved).to_vec())
    }
    fn list(&self, path: &[u8]) -> Option<Vec<Vec<u8>>> {
        note(&[b"open:", path]);
        let names = SCRIPT.with_borrow(|script| script.names.clone());
        for name in &names {
            note(&[b"read:", name]);
        }
        note(&[b"end"]);
        note(&[b"close"]);
        // readdir's C strings.
        Some(names.iter().map(|name| c_text(name).to_vec()).collect())
    }
    fn mime(&self, filename: &[u8]) -> Vec<u8> {
        note(&[b"mime:", filename]);
        SCRIPT.with_borrow(|script| script.mime.clone())
    }
    fn mime_exists(&self, filename: &[u8]) -> bool {
        note(&[b"mime-exists:", filename]);
        true
    }
    fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize) {
        note(&[b"random:", count.to_string().as_bytes()]);
        out[..count].fill(b'x');
        out[count] = 0;
    }
    fn modified_range(&self, if_range: &[u8], mtime: i64) -> bool {
        swoole_rs_http::conditional(if_range, 1, mtime, 0, self)
    }
    fn overflow(&self) {
        set_errno(libc::EOVERFLOW);
    }
}

/// libc's calendar, recorded by the oracle as it records its own.
impl Calendar for Scripted {
    fn parse(&self, date: &CStr, format: &CStr, tm: &mut Tm) -> bool {
        // SAFETY: two C strings and an exclusive tm.
        unsafe {
            oracle::swr_oracle_calendar_parse(
                date.as_ptr(),
                format.as_ptr(),
                ptr::from_mut(tm).cast(),
            )
        }
    }
    fn to_time(&self, tm: &mut Tm) -> i64 {
        // SAFETY: an exclusive, initialized tm.
        unsafe { oracle::swr_oracle_calendar_time(ptr::from_mut(tm).cast()) }
    }
    fn to_gmt(&self, time: i64) -> Option<Tm> {
        // SAFETY: null or libc's static tm, copied before another calendar call.
        unsafe { oracle::swr_oracle_calendar_gmt(time).cast::<Tm>().as_ref() }.copied()
    }
}

impl Effects for Scripted {}

/// A rewrite rule: pattern, replacement and whether it is a regex.
type Rule = (&'static [u8], &'static [u8], bool);

/// One server's configuration, given to both sides.
#[derive(Default)]
struct Config {
    autoindex: bool,
    location: bool,
    index: Option<Vec<&'static [u8]>>,
    timezone: c_long,
    rules: Vec<Rule>,
}

/// The pinned `StaticHandler` on its own server.
struct Upstream(*mut c_void);

impl Upstream {
    fn new(config: &Config, url: &[u8]) -> Self {
        let index: Vec<CString> = config.index.iter().flatten().map(|name| c(name)).collect();
        let index: Vec<*const c_char> = index.iter().map(|name| name.as_ptr()).collect();
        let texts: Vec<(CString, CString)> = config
            .rules
            .iter()
            .map(|(pattern, replacement, _)| (c(pattern), c(replacement)))
            .collect();
        let rules: Vec<oracle::Rule> = texts
            .iter()
            .zip(&config.rules)
            .map(|((pattern, replacement), rule)| oracle::Rule {
                pattern: pattern.as_ptr(),
                replacement: replacement.as_ptr(),
                is_regex: i32::from(rule.2),
            })
            .collect();
        let server = oracle::Server {
            autoindex: i32::from(config.autoindex),
            location: i32::from(config.location),
            has_index: i32::from(config.index.is_some()),
            timezone: config.timezone,
            index: index.as_ptr(),
            index_count: index.len(),
            rules: rules.as_ptr(),
            rule_count: rules.len(),
        };
        // SAFETY: the server's strings outlive the call, which copies them.
        Self(unsafe {
            oracle::swr_oracle_static_new(&raw const server, url.as_ptr().cast(), url.len())
        })
    }
    fn try_serve(&self) -> bool {
        // SAFETY: a live handle.
        unsafe { oracle::swr_oracle_static_try_serve(self.0) != 0 }
    }
    fn try_serve_index_file(&self) -> bool {
        // SAFETY: a live handle.
        unsafe { oracle::swr_oracle_static_try_serve_index_file(self.0) != 0 }
    }
    fn get_dir_files(&self) -> bool {
        // SAFETY: a live handle.
        unsafe { oracle::swr_oracle_static_get_dir_files(self.0) != 0 }
    }
    fn parse_range(&self, range: Option<&[u8]>, if_range: Option<&[u8]>) {
        let (range, if_range) = (range.map(c), if_range.map(c));
        let text = |value: &Option<CString>| value.as_ref().map_or(ptr::null(), |v| v.as_ptr());
        // SAFETY: a live handle and NUL-terminated headers (or null).
        unsafe { oracle::swr_oracle_static_parse_range(self.0, text(&range), text(&if_range)) };
    }
    fn is_modified(&self, date: &[u8]) -> bool {
        // SAFETY: a live handle and the date's bytes.
        unsafe {
            oracle::swr_oracle_static_is_modified(self.0, date.as_ptr().cast(), date.len()) != 0
        }
    }
    fn is_modified_range(&self, date: &[u8]) -> bool {
        // SAFETY: a live handle and the date's bytes.
        unsafe {
            oracle::swr_oracle_static_is_modified_range(self.0, date.as_ptr().cast(), date.len())
                != 0
        }
    }
    fn filename(&self) -> Vec<u8> {
        // SAFETY: a live handle and collector.
        lines(|line, out| unsafe { oracle::swr_oracle_static_filename(self.0, line, out) }).concat()
    }
    /// The page `make_index_page` appends to `buffer`.
    fn index_page(&self, buffer: *mut c_void) -> Vec<u8> {
        // SAFETY: a live handle, buffer and collector.
        lines(|line, out| unsafe {
            oracle::swr_oracle_static_index_page(self.0, buffer, line, out);
        })
        .concat()
    }
    /// The state lines, and whether the MIME and content-type references are upstream's.
    fn snapshot(&self) -> (Vec<Vec<u8>>, bool) {
        let mut identity = 0;
        // SAFETY: a live handle and collector.
        let lines = lines(|line, out| {
            identity = unsafe { oracle::swr_oracle_static_snapshot(self.0, line, out) }
        });
        (lines, identity != 0)
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        // SAFETY: freed once.
        unsafe { oracle::swr_oracle_static_free(self.0) };
    }
}

/// The head of a native `Server` (see `view`).
#[repr(C)]
struct Head {
    state: *mut ServerState,
    config: *mut ServerConfig,
}

/// A server with only the state and configuration a `StaticHandler` reads.
struct Server(Box<Head>);

impl Server {
    fn new(config: &Config) -> Self {
        let state = Box::into_raw(Box::new(ServerState::new()));
        let mut server_config = ServerConfig::default();
        server_config.http_autoindex = config.autoindex;
        server_config.timezone_ = config.timezone;
        let server = Self(Box::new(Head {
            state,
            config: Box::into_raw(Box::new(server_config)),
        }));
        let root = c"/root".as_ptr().expose_provenance() as u64;
        // SAFETY: the live state and a NUL-terminated root.
        unsafe { native_fact(&*state, OP_ST_SET_ROOT, root) };
        if config.location {
            server.strings(0, 4, b"/");
        }
        if let Some(index) = &config.index {
            server.strings(1, 2, b"");
            for name in index {
                server.strings(1, 6, name);
            }
        }
        server
    }
    /// `swr_v1_server_static_config(kind, operation, bytes)`.
    fn strings(&self, kind: u32, operation: u32, bytes: &[u8]) {
        // SAFETY: the live state and the bytes.
        unsafe {
            crate::static_config::swr_v1_server_static_config(
                &*self.0.state,
                kind,
                operation,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
    }
    fn rules(&self, rules: &[Rule]) {
        for &(pattern, replacement, regex) in rules {
            // Rust's rules put a regex between `~` delimiters.
            let pattern = if regex {
                [b"~", pattern, b"~"].concat()
            } else {
                pattern.to_vec()
            };
            // SAFETY: the live state.
            crate::static_config::add_rewrite_rule(
                unsafe { &*self.0.state },
                &pattern,
                replacement,
            );
        }
    }
    fn handler(&self, url: &[u8]) -> StaticHandler<Scripted> {
        // SAFETY: the server outlives the handler.
        unsafe {
            StaticHandler::with_effects(ptr::from_ref(&*self.0).cast_mut().cast(), url, Scripted)
        }
        .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // SAFETY: each allocated in `new`, freed once.
        unsafe { drop(Box::from_raw(self.0.state)) };
        // SAFETY: as above.
        unsafe { drop(Box::from_raw(self.0.config)) };
    }
}

/// The context gate's rewrite rule modes.
fn rules(mode: u32) -> Vec<Rule> {
    match mode {
        1 => vec![(b"/file", b"/normal", false)],
        2 => vec![(b"^/file", b"/normal", true)],
        3 => vec![
            (b"/missing", b"/skip", false),
            (b"/file", b"/normal", false),
        ],
        4 => vec![
            (b"^/missing", b"/skip", true),
            (b"^/file", b"/normal", true),
        ],
        5 => vec![(b"^/file", b"/file", true), (b"/file", b"/normal", false)],
        6 => vec![
            (b"/file", b"/file", false),
            (b"^/file", b"/should-not-apply", true),
        ],
        _ => vec![],
    }
}

struct FileCase<'a> {
    size: usize,
    url: &'a [u8],
    range: Option<&'a [u8]>,
    date: Option<&'a [u8]>,
    mime: &'a [u8],
    repeat: u32,
    rewrite: u32,
    locations: bool,
}

impl FileCase<'_> {
    fn script(&self) {
        let link: &[u8] = if self.url == b"/alias.txt" {
            b"/root/alias.txt"
        } else {
            b""
        };
        script(Script {
            file_size: self.size,
            file_time: 1_700_000_000,
            link: link.to_vec(),
            canonical: if link.is_empty() {
                vec![]
            } else {
                b"/root/file.txt".to_vec()
            },
            mime: self.mime.to_vec(),
            ..Script::default()
        });
    }
    /// The second `parse_range`'s header.
    fn repeated(&self) -> Option<&'static [u8]> {
        match self.repeat {
            1 => Some(b"bytes=0-0"),
            2 => None,
            _ => Some(b""),
        }
    }
    fn upstream(&self) -> Vec<Vec<u8>> {
        self.script();
        let config = Config {
            location: self.locations,
            rules: rules(self.rewrite),
            ..Config::default()
        };
        let handler = Upstream::new(&config, self.url);
        assert!(handler.try_serve(), "original normal file selection");
        assert!(
            handler.try_serve(),
            "original repeated normal file selection"
        );
        set_errno(libc::EINTR);
        handler.parse_range(self.range, self.date);
        if self.repeat != 0 {
            handler.parse_range(self.repeated(), None);
        }
        let (mut lines, identity) = handler.snapshot();
        assert!(
            identity,
            "the original global MIME reference and scoped content references"
        );
        let mut trace = upstream_trace();
        // SAFETY: no arguments.
        if unsafe { oracle::swr_oracle_static_arguments_left_to_right() } == 0 {
            // The first part header's boundary and MIME type, in source order.
            for i in 1..trace.len() {
                if trace[i].starts_with(b"random:") && trace[i - 1].starts_with(b"mime:") {
                    trace.swap(i - 1, i);
                }
            }
        }
        lines.extend(trace);
        lines
    }
    fn candidate(&self) -> Vec<Vec<u8>> {
        self.script();
        let server = Server::new(&Config {
            location: self.locations,
            ..Config::default()
        });
        let handler = server.handler(self.url);
        // Rules set after the handler is made are still its server's.
        server.rules(&rules(self.rewrite));
        assert!(handler.try_serve(), "selected normal file selection");
        assert!(
            handler.try_serve(),
            "selected repeated normal file selection"
        );
        set_errno(libc::EINTR);
        handler.parse_range(self.range, self.date);
        if self.repeat != 0 {
            handler.parse_range(self.repeated(), None);
        }
        let tasks = handler.tasks();
        let mut lines: Vec<Vec<u8>> = tasks
            .iter()
            .map(|task| {
                [
                    format!("task:{}:{}:", task.offset, task.length).as_bytes(),
                    c_text(&task.part_header),
                ]
                .concat()
            })
            .collect();
        lines.push([b"type:".as_slice(), &handler.content_type()].concat());
        let modified = handler.is_modified(b"Tue, 14 Nov 2023 22:13:20 GMT");
        lines.push(format!("modified:{}", u8::from(modified)).into_bytes());
        lines.push([b"end:".as_slice(), &handler.end_part()].concat());
        lines.push([b"filename:".as_slice(), c_text(&handler.filename())].concat());
        lines.push([b"date:".as_slice(), &handler.date_last_modified()].concat());
        lines.push(format!("length:{}", handler.content_length()).into_bytes());
        lines.push(format!("status:{}", handler.status()).into_bytes());
        // The original's MIME reference read.
        let _mime = handler.mimetype();
        lines.push(format!("errno:{}", os_errno()).into_bytes());
        drop(handler);
        for task in &tasks {
            assert!(
                task.part_header.contains(&0),
                "copied task survives complete context destruction"
            );
        }
        lines.extend(trace());
        lines
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
fn former_context_gate_files() {
    let ranges: [Option<&[u8]>; 7] = [
        None,
        Some(b""),
        Some(b"bytes=0-0"),
        Some(b"bytes=0-"),
        Some(b"bytes=-1"),
        Some(b"bytes=0-0,1-2"),
        Some(b"bytes=1-3,5-7"),
    ];
    let dates: [Option<&[u8]>; 3] = [
        None,
        Some(b"Tue, 14 Nov 2023 22:13:20 GMT"),
        Some(b"Wed, 15 Nov 2023 22:13:20 GMT"),
    ];
    for size in [0, 1, 17, 4096] {
        for url in [b"/file.txt".as_slice(), b"/file%20name.txt?version=1"] {
            for range in ranges {
                for date in dates {
                    for mime in [b"text/plain".as_slice(), b"application/octet-stream"] {
                        for repeat in 0..4 {
                            for rewrite in 0..7 {
                                let case = FileCase {
                                    size,
                                    url,
                                    range,
                                    date,
                                    mime,
                                    repeat,
                                    rewrite,
                                    locations: false,
                                };
                                assert_eq!(
                                    case.upstream(),
                                    case.candidate(),
                                    "complete context tasks, text, accounting, status, date, \
                                     conditional, effects and errno"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    // Ordinary within-root symlink and configured-location anchors.
    for url in [b"/file.txt".as_slice(), b"/alias.txt"] {
        for locations in [false, true] {
            for size in [1, 17] {
                let case = FileCase {
                    size,
                    url,
                    range: None,
                    date: None,
                    mime: b"text/plain",
                    repeat: 0,
                    rewrite: 0,
                    locations,
                };
                assert_eq!(
                    case.upstream(),
                    case.candidate(),
                    "symlink and location anchors"
                );
            }
        }
    }
}

fn names(names: &[&str]) -> Vec<Vec<u8>> {
    names.iter().map(|name| name.as_bytes().to_vec()).collect()
}

#[test]
#[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
fn former_context_gate_directories() {
    let mut listings = vec![
        names(&[".", ".."]),
        names(&["z.txt", ".", "a.txt", "..", "z.txt"]),
        names(&[
            "index.html",
            "README.md",
            ".",
            "..",
            "a b",
            "a&b",
            "<name>",
            "é.txt",
        ]),
        [vec![b'x'; 200]]
            .into_iter()
            .chain(names(&["a.txt", ".", ".."]))
            .collect(),
    ];
    for count in [1, 17, 65, 257, 1024] {
        let mut entries = names(&[".", ".."]);
        entries.extend(
            (0..count)
                .rev()
                .map(|i| format!("file-{i}.txt").into_bytes()),
        );
        entries.push(entries.last().unwrap().clone());
        listings.push(entries);
    }
    for entries in &listings {
        for path in ["/", "/normal", "/normal/"] {
            for mode in 0..6 {
                let index: Option<Vec<&'static [u8]>> = match mode {
                    1 | 5 => Some(vec![b"missing"]),
                    2 => Some(vec![b"README.md", b"index.html", b"a.txt"]),
                    3 => Some(vec![b"a.txt", b"index.html", b"README.md"]),
                    4 => Some(vec![]),
                    _ => None,
                };
                let config = Config {
                    autoindex: mode != 5,
                    index,
                    ..Config::default()
                };
                for capacity in [16, 128, 32768] {
                    script(Script {
                        file_size: 100,
                        file_time: 1_700_000_000,
                        directory: [b"/root", path.as_bytes()].concat(),
                        mime: b"text/plain".to_vec(),
                        names: entries.clone(),
                        ..Script::default()
                    });
                    let old = Upstream::new(&config, path.as_bytes());
                    let server = Server::new(&config);
                    let selected = server.handler(path.as_bytes());
                    assert_eq!(
                        old.try_serve(),
                        selected.try_serve(),
                        "directory context selection"
                    );
                    assert_eq!(
                        old.try_serve_index_file(),
                        selected.try_serve_index_file(),
                        "real index path selection"
                    );
                    assert_eq!(
                        old.get_dir_files(),
                        selected
                            .context
                            .borrow_mut()
                            .get_dir_files(&selected.effects),
                        "directory context scan"
                    );
                    assert_eq!(
                        old.get_dir_files(),
                        selected
                            .context
                            .borrow_mut()
                            .get_dir_files(&selected.effects),
                        "directory context cache"
                    );
                    // SAFETY: freed below.
                    let buffer = unsafe { oracle::swr_oracle_static_buffer_new(capacity) };
                    assert_eq!(
                        old.index_page(buffer),
                        selected.index_page(),
                        "directory page bytes"
                    );
                    assert_eq!(
                        old.index_page(buffer),
                        selected.index_page(),
                        "repeated directory page bytes"
                    );
                    // SAFETY: freed once.
                    unsafe { oracle::swr_oracle_static_buffer_free(buffer) };
                    assert_eq!(
                        old.filename(),
                        c_text(&selected.filename()),
                        "selected complete filename backing"
                    );
                    assert_eq!(
                        upstream_trace(),
                        trace(),
                        "complete directory external effect ordering"
                    );
                }
            }
        }
    }
}

/// The conditional gate's dates: its seeds, then every proper prefix of each.
fn conditional_dates() -> Vec<Vec<u8>> {
    let mut dates: Vec<Vec<u8>> = [
        "",
        "invalid",
        "Sun, 06 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 UTC",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
        "Sun, 06 Nov 1994 08:49:37 GMT suffix",
        "Mon, 29 Feb 2000 00:00:00 GMT",
        "Fri, 29 Feb 1900 00:00:00 GMT",
        "Sun, 32 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 25:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:60 GMT",
        "Sunday, 06-Nov-68 08:49:37 GMT",
        "Sunday, 06-Nov-69 08:49:37 GMT",
    ]
    .iter()
    .map(|date| date.as_bytes().to_vec())
    .collect();
    dates.extend([vec![b'x'; 63], vec![b'x'; 64], vec![b'x'; 1000]]);
    dates.push(b"Sun, 06 Nov 1994 08:49:37 GMT\0ignored"[..34].to_vec());
    dates.push(b"Sun, 06 Nov\0 1994 08:49:37 GMT"[..29].to_vec());
    let seeds = dates.len();
    for i in 0..seeds {
        let value = dates[i].clone();
        dates.extend((1..value.len()).map(|length| value[..length].to_vec()));
    }
    dates
}

/// One verdict, its errno and its calendar events, from errno `EINTR`.
fn observed(verdict: impl FnOnce() -> bool) -> (bool, i32, Vec<Vec<u8>>) {
    calendar_events();
    set_errno(libc::EINTR);
    let value = verdict();
    let errno = os_errno();
    (value, errno, calendar_events())
}

/// `is_modified` (kind 0) or `is_modified_range` (kind 1) of `date` on a file of
/// `file` mtime, with timezone `offset`, on both sides. The candidate's If-Range
/// verdict is `parse_range`'s: a 206 for `bytes=0-0` keeps the range.
fn compare_conditional(old: &Upstream, server: &Server, file: i64, date: &[u8], kind: u32) {
    let original = if kind == 0 {
        observed(|| old.is_modified(date))
    } else {
        observed(|| old.is_modified_range(date))
    };
    let candidate = if kind == 0 {
        let handler = server.handler(b"/file.txt");
        assert!(handler.try_serve(), "file selection");
        observed(|| handler.is_modified(date))
    } else {
        conditional_script(file);
        let handler = server.handler(b"/file.txt");
        assert!(handler.try_serve(), "file selection");
        observed(|| {
            handler.parse_range(Some(b"bytes=0-0"), Some(date));
            handler.status() == 200
        })
    };
    assert_eq!(
        original, candidate,
        "conditional verdict, errno and calendar calls"
    );
}

fn conditional_script(file: i64) {
    script(Script {
        file_size: 17,
        file_time: file,
        mime: b"text/plain".to_vec(),
        ..Script::default()
    });
}

/// The conditional gate in this process's `TZ` (see [`former_conditional_gate`]).
fn conditional_cases() {
    // Warm libc's timezone state before errno and call-order observations.
    // SAFETY: a zeroed tm is valid, and nothing else in this process uses libc's
    // calendar statics meanwhile.
    unsafe {
        let mut warm: libc::tm = core::mem::zeroed();
        warm.tm_year = 94;
        warm.tm_mon = 10;
        warm.tm_mday = 6;
        libc::mktime(&raw mut warm);
        libc::gmtime(&0);
    }
    let dates = conditional_dates();
    for file in [
        -2_208_988_800,
        -1,
        0,
        784_111_776,
        784_111_777,
        784_111_778,
        4_102_444_800,
    ] {
        for offset in [-3600, 0, 32400] {
            conditional_script(file);
            let config = Config {
                timezone: offset,
                ..Config::default()
            };
            let old = Upstream::new(&config, b"/file.txt");
            assert!(old.try_serve(), "original file selection");
            let server = Server::new(&config);
            for date in &dates {
                for kind in [0, 1] {
                    compare_conditional(&old, &server, file, date, kind);
                }
            }
        }
    }
}

/// The conditional gate's random dates: `std::mt19937(42819)`, each date's length
/// `% 100`, then its bytes `% 128`.
fn random_dates() -> Vec<Vec<u8>> {
    let mut values = vec![0u32; 2048 * 101];
    // SAFETY: `values.len()` writable outputs.
    unsafe { oracle::swr_oracle_mt19937(42819, values.as_mut_ptr(), values.len()) };
    let mut values = values.into_iter();
    let mut next = || values.next().unwrap();
    (0..2048)
        .map(|_| {
            let length = next() % 100;
            (0..length)
                .map(|_| u8::try_from(next() % 128).unwrap())
                .collect()
        })
        .collect()
}

/// The zone sweep runs in a child per `TZ`, since the environment is
/// process-wide; this process then runs the random dates and the two cases
/// where the original's arithmetic misbehaves and only the correction runs.
#[test]
#[cfg_attr(miri, ignore = "Miri has no posix_spawnattr_init")]
fn former_conditional_gate() {
    if std::env::var_os("SWR_CONDITIONAL_ZONE").is_some() {
        conditional_cases();
        return;
    }
    for zone in ["UTC0", "EST5EDT", "JST-9"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "static_handler::tests::former_conditional_gate"])
            .env("SWR_CONDITIONAL_ZONE", zone)
            .env("TZ", zone)
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "conditional cases in {zone}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    conditional_script(784_111_777);
    let old = Upstream::new(&Config::default(), b"/file.txt");
    assert!(old.try_serve(), "original file selection");
    let server = Server::new(&Config::default());
    for date in random_dates() {
        for kind in [0, 1] {
            compare_conditional(&old, &server, 784_111_777, &date, kind);
        }
    }
    // The original dereferences gmtime's null for an unrepresentable mtime.
    conditional_script(i64::MAX);
    let handler = server.handler(b"/file.txt");
    assert!(handler.try_serve(), "file selection");
    let (modified, _, events) = observed(|| {
        handler.parse_range(Some(b"bytes=0-0"), Some(b"Sun, 06 Nov 1994 08:49:37 GMT"));
        handler.status() == 200
    });
    assert!(!modified, "an unrepresentable mtime is not modified");
    assert!(
        events.len() == 2 && events[1].windows(6).any(|w| w == b":null:"),
        "one parse, then gmtime's null"
    );
    // The original's timezone subtraction overflows.
    conditional_script(0);
    let server = Server::new(&Config {
        timezone: c_long::MIN,
        ..Config::default()
    });
    let handler = server.handler(b"/file.txt");
    assert!(handler.try_serve(), "file selection");
    assert!(
        !handler.is_modified(b"Sun, 06 Nov 1994 08:49:37 GMT"),
        "an overflowing timezone is not modified"
    );
}
