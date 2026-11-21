//! Upstream's `mime_type` table (`src/protocol/mime_type.cc`): the catalog, the
//! process-wide map PHP's `swoole_mime_type_*` functions change, and lookup by a
//! filename's lowercased last suffix.
use core::ffi::CStr;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

/// What `get` answers for an unknown suffix.
pub const FALLBACK: &CStr = c"application/octet-stream";

static MAP: LazyLock<Mutex<BTreeMap<Vec<u8>, CString>>> = LazyLock::new(|| {
    Mutex::new(
        CATALOG
            .iter()
            .filter_map(|&(suffix, kind)| Some((suffix.to_vec(), CString::new(kind).ok()?)))
            .collect(),
    )
});

fn map() -> MutexGuard<'static, BTreeMap<Vec<u8>, CString>> {
    MAP.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bytes after the last dot (all of them without one), ASCII-lowercased:
/// upstream's `std::transform` of each `char` through C `tolower` in the "C"
/// locale PHP starts in.
fn suffix(filename: &[u8]) -> Vec<u8> {
    let start = filename
        .iter()
        .rposition(|&c| c == b'.')
        .map_or(0, |n| n + 1);
    filename
        .get(start..)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// `mime_type::add`: false, changing nothing, when the suffix is known.
#[must_use]
pub fn add(suffix: &[u8], kind: &CStr) -> bool {
    let mut map = map();
    if map.contains_key(suffix) {
        return false;
    }
    map.insert(suffix.to_vec(), kind.to_owned());
    true
}

/// `mime_type::set`.
pub fn set(suffix: &[u8], kind: &CStr) {
    map().insert(suffix.to_vec(), kind.to_owned());
}

/// `mime_type::del`: whether the suffix was known.
#[must_use]
pub fn del(suffix: &[u8]) -> bool {
    map().remove(suffix).is_some()
}

/// `mime_type::get`: a copy of the type, as callers take before the table changes.
#[must_use]
pub fn get(filename: &[u8]) -> Vec<u8> {
    map()
        .get(&suffix(filename))
        .map_or(FALLBACK, CString::as_c_str)
        .to_bytes()
        .to_vec()
}

/// `mime_type::exists`.
#[must_use]
pub fn exists(filename: &[u8]) -> bool {
    map().contains_key(&suffix(filename))
}

/// `mime_type::list`'s types, in suffix order.
pub fn each(mut f: impl FnMut(&CStr)) {
    for kind in map().values() {
        f(kind);
    }
}

// Copied byte-for-byte from the pinned reference.
static CATALOG: &[(&[u8], &[u8])] = &[
    (b"ez", b"application/andrew-inset"),
    (b"aw", b"application/applixware"),
    (b"atom", b"application/atom+xml"),
    (b"atomcat", b"application/atomcat+xml"),
    (b"atomsvc", b"application/atomsvc+xml"),
    (b"bdoc", b"application/bdoc"),
    (b"ccxml", b"application/ccxml+xml"),
    (b"cdmia", b"application/cdmi-capability"),
    (b"cdmic", b"application/cdmi-container"),
    (b"cdmid", b"application/cdmi-domain"),
    (b"cdmio", b"application/cdmi-object"),
    (b"cdmiq", b"application/cdmi-queue"),
    (b"cu", b"application/cu-seeme"),
    (b"mpd", b"application/dash+xml"),
    (b"davmount", b"application/davmount+xml"),
    (b"dbk", b"application/docbook+xml"),
    (b"dssc", b"application/dssc+der"),
    (b"xdssc", b"application/dssc+xml"),
    (b"ecma", b"application/ecmascript"),
    (b"emma", b"application/emma+xml"),
    (b"epub", b"application/epub+zip"),
    (b"exi", b"application/exi"),
    (b"pfr", b"application/font-tdpfr"),
    (b"woff", b"application/font-woff"),
    (b"geojson", b"application/geo+json"),
    (b"gml", b"application/gml+xml"),
    (b"gpx", b"application/gpx+xml"),
    (b"gxf", b"application/gxf"),
    (b"gz", b"application/gzip"),
    (b"hjson", b"application/hjson"),
    (b"stk", b"application/hyperstudio"),
    (b"ink", b"application/inkml+xml"),
    (b"inkml", b"application/inkml+xml"),
    (b"ipfix", b"application/ipfix"),
    (b"jar", b"application/java-archive"),
    (b"war", b"application/java-archive"),
    (b"ear", b"application/java-archive"),
    (b"apk", b"application/vnd.android.package-archive"),
    (b"ser", b"application/java-serialized-object"),
    (b"class", b"application/java-vm"),
    (b"js", b"application/javascript"),
    (b"mjs", b"application/javascript"),
    (b"json", b"application/json"),
    (b"map", b"application/json"),
    (b"json5", b"application/json5"),
    (b"jsonml", b"application/jsonml+json"),
    (b"jsonld", b"application/ld+json"),
    (b"lostxml", b"application/lost+xml"),
    (b"hqx", b"application/mac-binhex40"),
    (b"cpt", b"application/mac-compactpro"),
    (b"mads", b"application/mads+xml"),
    (b"webmanifest", b"application/manifest+json"),
    (b"mrc", b"application/marc"),
    (b"mrcx", b"application/marcxml+xml"),
    (b"ma", b"application/mathematica"),
    (b"nb", b"application/mathematica"),
    (b"mb", b"application/mathematica"),
    (b"mathml", b"application/mathml+xml"),
    (b"mbox", b"application/mbox"),
    (b"mscml", b"application/mediaservercontrol+xml"),
    (b"metalink", b"application/metalink+xml"),
    (b"meta4", b"application/metalink4+xml"),
    (b"mets", b"application/mets+xml"),
    (b"mods", b"application/mods+xml"),
    (b"m21", b"application/mp21"),
    (b"mp21", b"application/mp21"),
    (b"mp4s", b"application/mp4"),
    (b"m4p", b"application/mp4"),
    (b"doc", b"application/msword"),
    (
        b"docx",
        b"application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (b"dot", b"application/msword"),
    (b"mxf", b"application/mxf"),
    (b"bin", b"application/octet-stream"),
    (b"dms", b"application/octet-stream"),
    (b"lrf", b"application/octet-stream"),
    (b"mar", b"application/octet-stream"),
    (b"so", b"application/octet-stream"),
    (b"dist", b"application/octet-stream"),
    (b"distz", b"application/octet-stream"),
    (b"pkg", b"application/octet-stream"),
    (b"bpk", b"application/octet-stream"),
    (b"dump", b"application/octet-stream"),
    (b"elc", b"application/octet-stream"),
    (b"deploy", b"application/octet-stream"),
    (b"exe", b"application/octet-stream"),
    (b"dll", b"application/octet-stream"),
    (b"deb", b"application/octet-stream"),
    (b"dmg", b"application/octet-stream"),
    (b"iso", b"application/octet-stream"),
    (b"img", b"application/octet-stream"),
    (b"msi", b"application/octet-stream"),
    (b"msp", b"application/octet-stream"),
    (b"msm", b"application/octet-stream"),
    (b"buffer", b"application/octet-stream"),
    (b"oda", b"application/oda"),
    (b"opf", b"application/oebps-package+xml"),
    (b"ogx", b"application/ogg"),
    (b"omdoc", b"application/omdoc+xml"),
    (b"onetoc", b"application/onenote"),
    (b"onetoc2", b"application/onenote"),
    (b"onetmp", b"application/onenote"),
    (b"onepkg", b"application/onenote"),
    (b"oxps", b"application/oxps"),
    (b"xer", b"application/patch-ops-error+xml"),
    (b"pdf", b"application/pdf"),
    (b"pgp", b"application/pgp-encrypted"),
    (b"asc", b"application/pgp-signature"),
    (b"sig", b"application/pgp-signature"),
    (b"prf", b"application/pics-rules"),
    (b"p10", b"application/pkcs10"),
    (b"p7m", b"application/pkcs7-mime"),
    (b"p7c", b"application/pkcs7-mime"),
    (b"p7s", b"application/pkcs7-signature"),
    (b"p8", b"application/pkcs8"),
    (b"ac", b"application/pkix-attr-cert"),
    (b"cer", b"application/pkix-cert"),
    (b"crl", b"application/pkix-crl"),
    (b"pkipath", b"application/pkix-pkipath"),
    (b"pki", b"application/pkixcmp"),
    (b"pls", b"application/pls+xml"),
    (b"ai", b"application/postscript"),
    (b"eps", b"application/postscript"),
    (b"ps", b"application/postscript"),
    (b"pskcxml", b"application/pskc+xml"),
    (b"raml", b"application/raml+yaml"),
    (b"rdf", b"application/rdf+xml"),
    (b"rif", b"application/reginfo+xml"),
    (b"rnc", b"application/relax-ng-compact-syntax"),
    (b"rl", b"application/resource-lists+xml"),
    (b"rld", b"application/resource-lists-diff+xml"),
    (b"rs", b"application/rls-services+xml"),
    (b"gbr", b"application/rpki-ghostbusters"),
    (b"mft", b"application/rpki-manifest"),
    (b"roa", b"application/rpki-roa"),
    (b"rsd", b"application/rsd+xml"),
    (b"rss", b"application/rss+xml"),
    (b"rtf", b"application/rtf"),
    (b"sbml", b"application/sbml+xml"),
    (b"scq", b"application/scvp-cv-request"),
    (b"scs", b"application/scvp-cv-response"),
    (b"spq", b"application/scvp-vp-request"),
    (b"spp", b"application/scvp-vp-response"),
    (b"sdp", b"application/sdp"),
    (b"setpay", b"application/set-payment-initiation"),
    (b"setreg", b"application/set-registration-initiation"),
    (b"shf", b"application/shf+xml"),
    (b"smi", b"application/smil+xml"),
    (b"smil", b"application/smil+xml"),
    (b"rq", b"application/sparql-query"),
    (b"srx", b"application/sparql-results+xml"),
    (b"gram", b"application/srgs"),
    (b"grxml", b"application/srgs+xml"),
    (b"sru", b"application/sru+xml"),
    (b"ssdl", b"application/ssdl+xml"),
    (b"ssml", b"application/ssml+xml"),
    (b"tei", b"application/tei+xml"),
    (b"teicorpus", b"application/tei+xml"),
    (b"tfi", b"application/thraud+xml"),
    (b"tsd", b"application/timestamped-data"),
    (b"vxml", b"application/voicexml+xml"),
    (b"wasm", b"application/wasm"),
    (b"wgt", b"application/widget"),
    (b"hlp", b"application/winhlp"),
    (b"wsdl", b"application/wsdl+xml"),
    (b"wspolicy", b"application/wspolicy+xml"),
    (b"xaml", b"application/xaml+xml"),
    (b"xdf", b"application/xcap-diff+xml"),
    (b"xenc", b"application/xenc+xml"),
    (b"xhtml", b"application/xhtml+xml"),
    (b"xht", b"application/xhtml+xml"),
    (b"xml", b"application/xml"),
    (b"xsl", b"application/xml"),
    (b"xsd", b"application/xml"),
    (b"rng", b"application/xml"),
    (b"dtd", b"application/xml-dtd"),
    (b"xop", b"application/xop+xml"),
    (b"xpl", b"application/xproc+xml"),
    (b"xslt", b"application/xslt+xml"),
    (b"xspf", b"application/xspf+xml"),
    (b"mxml", b"application/xv+xml"),
    (b"xhvml", b"application/xv+xml"),
    (b"xvml", b"application/xv+xml"),
    (b"xvm", b"application/xv+xml"),
    (b"yang", b"application/yang"),
    (b"yin", b"application/yin+xml"),
    (b"zip", b"application/zip"),
    (b"*3gpp", b"audio/3gpp"),
    (b"adp", b"audio/adpcm"),
    (b"au", b"audio/basic"),
    (b"snd", b"audio/basic"),
    (b"mid", b"audio/midi"),
    (b"midi", b"audio/midi"),
    (b"kar", b"audio/midi"),
    (b"rmi", b"audio/midi"),
    (b"*mp3", b"audio/mp3"),
    (b"m4a", b"audio/mp4"),
    (b"mp4a", b"audio/mp4"),
    (b"mpga", b"audio/mpeg"),
    (b"mp2", b"audio/mpeg"),
    (b"mp2a", b"audio/mpeg"),
    (b"mp3", b"audio/mpeg"),
    (b"m2a", b"audio/mpeg"),
    (b"m3a", b"audio/mpeg"),
    (b"oga", b"audio/ogg"),
    (b"ogg", b"audio/ogg"),
    (b"spx", b"audio/ogg"),
    (b"s3m", b"audio/s3m"),
    (b"sil", b"audio/silk"),
    (b"wav", b"audio/wav"),
    (b"*wav", b"audio/wave"),
    (b"weba", b"audio/webm"),
    (b"xm", b"audio/xm"),
    (b"ttc", b"font/collection"),
    (b"otf", b"font/otf"),
    (b"ttf", b"font/ttf"),
    (b"*woff", b"font/woff"),
    (b"woff2", b"font/woff2"),
    (b"apng", b"image/apng"),
    (b"bmp", b"image/bmp"),
    (b"cgm", b"image/cgm"),
    (b"g3", b"image/g3fax"),
    (b"gif", b"image/gif"),
    (b"ief", b"image/ief"),
    (b"jp2", b"image/jp2"),
    (b"jpg2", b"image/jp2"),
    (b"jpeg", b"image/jpeg"),
    (b"jpg", b"image/jpeg"),
    (b"jpe", b"image/jpeg"),
    (b"jpm", b"image/jpm"),
    (b"jpx", b"image/jpx"),
    (b"jpf", b"image/jpx"),
    (b"ktx", b"image/ktx"),
    (b"png", b"image/png"),
    (b"sgi", b"image/sgi"),
    (b"svg", b"image/svg+xml"),
    (b"svgz", b"image/svg+xml"),
    (b"tiff", b"image/tiff"),
    (b"tif", b"image/tiff"),
    (b"webp", b"image/webp"),
    (
        b"disposition-notification",
        b"message/disposition-notification",
    ),
    (b"u8msg", b"message/global"),
    (b"u8dsn", b"message/global-delivery-status"),
    (b"u8mdn", b"message/global-disposition-notification"),
    (b"u8hdr", b"message/global-headers"),
    (b"eml", b"message/rfc822"),
    (b"mime", b"message/rfc822"),
    (b"gltf", b"model/gltf+json"),
    (b"glb", b"model/gltf-binary"),
    (b"igs", b"model/iges"),
    (b"iges", b"model/iges"),
    (b"msh", b"model/mesh"),
    (b"mesh", b"model/mesh"),
    (b"silo", b"model/mesh"),
    (b"wrl", b"model/vrml"),
    (b"vrml", b"model/vrml"),
    (b"x3db", b"model/x3d+binary"),
    (b"x3dbz", b"model/x3d+binary"),
    (b"x3dv", b"model/x3d+vrml"),
    (b"x3dvz", b"model/x3d+vrml"),
    (b"x3d", b"model/x3d+xml"),
    (b"x3dz", b"model/x3d+xml"),
    (b"appcache", b"text/cache-manifest"),
    (b"manifest", b"text/cache-manifest"),
    (b"ics", b"text/calendar"),
    (b"ifb", b"text/calendar"),
    (b"coffee", b"text/coffeescript"),
    (b"litcoffee", b"text/coffeescript"),
    (b"css", b"text/css"),
    (b"csv", b"text/csv"),
    (b"html", b"text/html"),
    (b"htm", b"text/html"),
    (b"shtml", b"text/html"),
    (b"jade", b"text/jade"),
    (b"jsx", b"text/jsx"),
    (b"less", b"text/less"),
    (b"markdown", b"text/markdown"),
    (b"md", b"text/markdown"),
    (b"mml", b"text/mathml"),
    (b"n3", b"text/n3"),
    (b"txt", b"text/plain"),
    (b"text", b"text/plain"),
    (b"conf", b"text/plain"),
    (b"def", b"text/plain"),
    (b"list", b"text/plain"),
    (b"log", b"text/plain"),
    (b"in", b"text/plain"),
    (b"ini", b"text/plain"),
    (b"rtx", b"text/richtext"),
    (b"*rtf", b"text/rtf"),
    (b"sgml", b"text/sgml"),
    (b"sgm", b"text/sgml"),
    (b"shex", b"text/shex"),
    (b"slim", b"text/slim"),
    (b"slm", b"text/slim"),
    (b"stylus", b"text/stylus"),
    (b"styl", b"text/stylus"),
    (b"tsv", b"text/tab-separated-values"),
    (b"t", b"text/troff"),
    (b"tr", b"text/troff"),
    (b"roff", b"text/troff"),
    (b"man", b"text/troff"),
    (b"me", b"text/troff"),
    (b"ms", b"text/troff"),
    (b"ttl", b"text/turtle"),
    (b"uri", b"text/uri-list"),
    (b"uris", b"text/uri-list"),
    (b"urls", b"text/uri-list"),
    (b"vcard", b"text/vcard"),
    (b"vtt", b"text/vtt"),
    (b"*xml", b"text/xml"),
    (b"yaml", b"text/yaml"),
    (b"yml", b"text/yaml"),
    (b"3gp", b"video/3gpp"),
    (b"3gpp", b"video/3gpp"),
    (b"3g2", b"video/3gpp2"),
    (b"h261", b"video/h261"),
    (b"h263", b"video/h263"),
    (b"h264", b"video/h264"),
    (b"jpgv", b"video/jpeg"),
    (b"*jpm", b"video/jpm"),
    (b"jpgm", b"video/jpm"),
    (b"mj2", b"video/mj2"),
    (b"mjp2", b"video/mj2"),
    (b"ts", b"video/mp2t"),
    (b"mp4", b"video/mp4"),
    (b"mp4v", b"video/mp4"),
    (b"mpg4", b"video/mp4"),
    (b"mpeg", b"video/mpeg"),
    (b"mpg", b"video/mpeg"),
    (b"mpe", b"video/mpeg"),
    (b"m1v", b"video/mpeg"),
    (b"m2v", b"video/mpeg"),
    (b"ogv", b"video/ogg"),
    (b"qt", b"video/quicktime"),
    (b"mov", b"video/quicktime"),
    (b"webm", b"video/webm"),
    (b"aac", b"audio/aac"),
    (b"abw", b"application/x-abiword"),
    (b"arc", b"application/octet-stream"),
    (b"avi", b"video/x-msvideo"),
    (b"azw", b"application/vnd.amazon.ebook"),
    (b"bz", b"application/x-bzip"),
    (b"bz2", b"application/x-bzip2"),
    (b"csh", b"application/x-csh"),
    (b"eot", b"application/vnd.ms-fontobject"),
    (b"es", b"application/ecmascript"),
    (b"ico", b"image/x-icon"),
    (b"mpkg", b"application/vnd.apple.installer+xml"),
    (b"odp", b"application/vnd.oasis.opendocument.presentation"),
    (b"ods", b"application/vnd.oasis.opendocument.spreadsheet"),
    (b"odt", b"application/vnd.oasis.opendocument.text"),
    (b"ppt", b"application/vnd.ms-powerpoint"),
    (
        b"pptx",
        b"application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    (b"rar", b"application/x-rar-compressed"),
    (b"sh", b"application/x-sh"),
    (b"swf", b"application/x-shockwave-flash"),
    (b"tar", b"application/x-tar"),
    (b"vsd", b"application/vnd.visio"),
    (b"xls", b"application/vnd.ms-excel"),
    (
        b"xlsx",
        b"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (b"xul", b"application/vnd.mozilla.xul+xml"),
    (b"7z", b"application/x-7z-compressed"),
];

#[cfg(test)]
mod tests {
    //! Upstream's `core-tests/src/protocol/mime_type.cpp`, one test per `TEST`,
    //! with its inputs and expected values. Upstream runs them in order in one
    //! process; here they share the table under `SERIAL`, and `del` puts back
    //! the `json` it deletes so the order doesn't matter.
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn get() {
        let _serial = serial();
        let result = super::get(b"test.html.json");
        assert_eq!(result, b"application/json");
    }

    #[test]
    fn exists() {
        let _serial = serial();
        assert!(super::exists(b"test.html.json"));
    }

    #[test]
    fn set() {
        let _serial = serial();
        let test_mime_type = c"application/swoole-core-test";
        super::set(b"swoole_test", test_mime_type);

        let result = super::get(b"test.swoole_test");
        assert_eq!(result, test_mime_type.to_bytes());
    }

    #[test]
    fn add() {
        let _serial = serial();
        let test_mime_type = c"application/swoole-core-test2";
        assert!(super::add(b"swoole_test2", test_mime_type));
        assert!(!super::add(b"swoole_test2", test_mime_type));

        let result = super::get(b"test.swoole_test2");
        assert_eq!(result, test_mime_type.to_bytes());
    }

    #[test]
    fn del() {
        let _serial = serial();
        assert!(super::del(b"json"));
        assert!(!super::exists(b"test.html.json"));
        super::set(b"json", c"application/json");
    }

    #[test]
    fn suffix_is_the_last_one_lowercased() {
        assert_eq!(suffix(b"a.B.JSON"), b"json");
        assert_eq!(suffix(b"noDot"), b"nodot");
        assert_eq!(suffix(b"trailing."), b"");
        assert_eq!(suffix(b"x.\0Y"), b"\0y");
        assert_eq!(suffix(b"x.\xc0\xff"), b"\xc0\xff");
        assert_eq!(CATALOG.len(), 362);
    }

    /// The C ABI gate's lookups (`tests/modules/framing/gate.cc`).
    #[test]
    fn lookup_lowercases_and_an_empty_name_finds_nothing() {
        let _serial = serial();
        assert_eq!(super::get(b"a.html.JSON"), b"application/json");
        assert!(super::exists(b"a.html.JSON"));
        assert_eq!(super::get(b""), FALLBACK.to_bytes());
        assert!(!super::exists(b""));
    }
}
