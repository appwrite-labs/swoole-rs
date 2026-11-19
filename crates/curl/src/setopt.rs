//! `curl_setopt` and `curl_setopt_array` (#5131). The options that take a number,
//! a string or a blob are set here, through the typed accessors in
//! `native/hooks/curl_handle.cc` (`curl_easy_setopt` is variadic), as are the
//! list options, `CURLOPT_RETURNTRANSFER`, `CURLOPT_PRIVATE`, the file-handle
//! options, `CURLOPT_SHARE` and `CURLOPT_POSTFIELDS` (`postfields.rs`); the
//! callables and `CURLINFO_HEADER_OUT` are set in `callbacks.rs`.
//!
//! Each option is gated as the copies' `#if`s gated its `case`: by the libcurl
//! headers the extension was built against and by the PHP version, so one the
//! build lacks is unknown here too.
use super::callbacks::{self, Plain, Stream};
use super::php_handle::{PhpHandle, argument};
use std::ffi::{CString, c_int, c_long};
use swoole_rs_zend::sys::{PHP_VERSION_ID, zend_long};
use swoole_rs_zend::{Call, E_NOTICE, E_WARNING, Key, Value, ZStr, bailout};

unsafe extern "C" {
    // `LIBCURL_VERSION_NUM`, from the headers the extension was built against.
    safe static swoole_curl_header_version: u32;
    // Each returns the option's `CURLcode` and saves it as the handle's error.
    pub(super) safe fn swoole_curl_setopt_long(
        handle: PhpHandle<'_>,
        option: c_int,
        value: c_long,
    ) -> c_int;
    safe fn swoole_curl_setopt_off_t(handle: PhpHandle<'_>, option: c_int, value: i64) -> c_int;
    pub(super) safe fn swoole_curl_setopt_string(
        handle: PhpHandle<'_>,
        option: c_int,
        value: Option<&ZStr>,
    ) -> c_int;
    safe fn swoole_curl_setopt_blob(handle: PhpHandle<'_>, option: c_int, value: &ZStr) -> c_int;
    pub(super) safe fn swoole_curl_save_error(handle: PhpHandle<'_>, error: c_int);
    // `curl_slist_append`; false leaves the list as it was.
    safe fn swoole_curl_list_append(list: &mut List, value: &ZStr) -> bool;
    safe fn swoole_curl_list_free(list: List);
    // Sets a list option and gives the list to the handle's clone family, which frees it.
    safe fn swoole_curl_setopt_list(handle: PhpHandle<'_>, option: c_int, list: List) -> c_int;
    safe fn swoole_curl_set_write_method(handle: PhpHandle<'_>, method: c_int);
    // The stream resource's `FILE *` (and whether it was opened for writing), or
    // None after the error.
    safe fn swoole_curl_stream_file(value: &Value, writable: &mut bool) -> Option<StdioFile>;
    // The stream a file-handle option keeps.
    pub(super) safe fn swoole_curl_file_stream<'a>(
        handle: PhpHandle<'a>,
        option: c_int,
    ) -> &'a mut swoole_rs_zend::Owned;
    // Points the option's handler at the file, or back at its default without one.
    safe fn swoole_curl_set_file(
        handle: PhpHandle<'_>,
        option: c_int,
        file: Option<StdioFile>,
    ) -> c_int;
    // `CURLOPT_SHARE`: sets and keeps a `CurlShareHandle`, and ignores any other value.
    safe fn swoole_curl_set_share(handle: PhpHandle<'_>, value: &Value);
    // The handle's `private_data`, for `CURLINFO_PRIVATE`.
    pub(super) safe fn swoole_curl_private<'a>(
        handle: PhpHandle<'a>,
    ) -> &'a mut swoole_rs_zend::Owned;
    safe fn swoole_curl_open_basedir() -> bool;
    // Warns about a path outside open_basedir, which a user error handler can bail out of.
    pub(super) safe fn swoole_curl_allowed_path(path: &ZStr) -> bool;
    // The URL as `curl_init` sets it: file:// is turned off under open_basedir.
    safe fn swoole_curl_set_url(handle: PhpHandle<'_>, url: &ZStr) -> bool;
}

swoole_rs_zend::handlers! {
    zif_swoole_native_curl_setopt => setopt;
    zif_swoole_native_curl_setopt_array => setopt_array;
}

/// PHP's own options, outside curl.h (`curl_private.h`).
const CURLOPT_RETURNTRANSFER: zend_long = 19913;
const CURLOPT_BINARYTRANSFER: zend_long = 19914;
const CURLOPT_SAFE_UPLOAD: zend_long = -1;
/// `PHP_CURL_STDOUT` and `PHP_CURL_RETURN`: where the write handler sends the body.
const PHP_CURL_STDOUT: c_int = 0;
const PHP_CURL_RETURN: c_int = 4;
/// PHP's own `CURLINFO_HEADER_OUT`, as an option.
const CURLINFO_HEADER_OUT: zend_long = 2;
/// `CURLE_UNKNOWN_OPTION`.
const UNKNOWN_OPTION: c_int = 48;
/// `CURLPROTO_FILE` and `CURL_REDIR_POST_ALL`.
const CURLPROTO_FILE: zend_long = 1 << 10;
const CURL_REDIR_POST_ALL: zend_long = 7;

// `CURLOPT_*`, from curl.h.
const CURLOPT_ABSTRACT_UNIX_SOCKET: zend_long = 10264;
const CURLOPT_ACCEPTTIMEOUT_MS: zend_long = 212;
const CURLOPT_ACCEPT_ENCODING: zend_long = 10102;
const CURLOPT_ADDRESS_SCOPE: zend_long = 171;
const CURLOPT_ALTSVC: zend_long = 10287;
const CURLOPT_ALTSVC_CTRL: zend_long = 286;
const CURLOPT_APPEND: zend_long = 50;
const CURLOPT_AUTOREFERER: zend_long = 58;
const CURLOPT_AWS_SIGV4: zend_long = 10305;
const CURLOPT_BUFFERSIZE: zend_long = 98;
const CURLOPT_CAINFO: zend_long = 10065;
const CURLOPT_CAINFO_BLOB: zend_long = 40309;
const CURLOPT_CAPATH: zend_long = 10097;
const CURLOPT_CA_CACHE_TIMEOUT: zend_long = 321;
const CURLOPT_CERTINFO: zend_long = 172;
const CURLOPT_CONNECTTIMEOUT: zend_long = 78;
const CURLOPT_CONNECTTIMEOUT_MS: zend_long = 156;
const CURLOPT_CONNECT_ONLY: zend_long = 141;
const CURLOPT_CONNECT_TO: zend_long = 10243;
const CURLOPT_COOKIE: zend_long = 10022;
const CURLOPT_COOKIEFILE: zend_long = 10031;
const CURLOPT_COOKIEJAR: zend_long = 10082;
const CURLOPT_COOKIELIST: zend_long = 10135;
const CURLOPT_COOKIESESSION: zend_long = 96;
const CURLOPT_CRLF: zend_long = 27;
const CURLOPT_CRLFILE: zend_long = 10169;
const CURLOPT_CUSTOMREQUEST: zend_long = 10036;
const CURLOPT_DEFAULT_PROTOCOL: zend_long = 10238;
const CURLOPT_DIRLISTONLY: zend_long = 48;
const CURLOPT_DISALLOW_USERNAME_IN_URL: zend_long = 278;
const CURLOPT_DNS_CACHE_TIMEOUT: zend_long = 92;
const CURLOPT_DNS_INTERFACE: zend_long = 10221;
const CURLOPT_DNS_LOCAL_IP4: zend_long = 10222;
const CURLOPT_DNS_LOCAL_IP6: zend_long = 10223;
const CURLOPT_DNS_SERVERS: zend_long = 10211;
const CURLOPT_DNS_SHUFFLE_ADDRESSES: zend_long = 275;
const CURLOPT_DNS_USE_GLOBAL_CACHE: zend_long = 91;
const CURLOPT_DOH_SSL_VERIFYHOST: zend_long = 307;
const CURLOPT_DOH_SSL_VERIFYPEER: zend_long = 306;
const CURLOPT_DOH_SSL_VERIFYSTATUS: zend_long = 308;
const CURLOPT_DOH_URL: zend_long = 10279;
const CURLOPT_EGDSOCKET: zend_long = 10077;
const CURLOPT_EXPECT_100_TIMEOUT_MS: zend_long = 227;
const CURLOPT_FAILONERROR: zend_long = 45;
const CURLOPT_FILE: zend_long = 10001;
const CURLOPT_FILETIME: zend_long = 69;
const CURLOPT_FOLLOWLOCATION: zend_long = 52;
const CURLOPT_FORBID_REUSE: zend_long = 75;
const CURLOPT_FRESH_CONNECT: zend_long = 74;
const CURLOPT_FTPPORT: zend_long = 10017;
const CURLOPT_FTPSSLAUTH: zend_long = 129;
const CURLOPT_FTP_ACCOUNT: zend_long = 10134;
const CURLOPT_FTP_ALTERNATIVE_TO_USER: zend_long = 10147;
const CURLOPT_FTP_CREATE_MISSING_DIRS: zend_long = 110;
const CURLOPT_FTP_FILEMETHOD: zend_long = 138;
const CURLOPT_FTP_SKIP_PASV_IP: zend_long = 137;
const CURLOPT_FTP_SSL_CCC: zend_long = 154;
const CURLOPT_FTP_USE_EPRT: zend_long = 106;
const CURLOPT_FTP_USE_EPSV: zend_long = 85;
const CURLOPT_FTP_USE_PRET: zend_long = 188;
const CURLOPT_GSSAPI_DELEGATION: zend_long = 210;
const CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS: zend_long = 271;
const CURLOPT_HAPROXYPROTOCOL: zend_long = 274;
const CURLOPT_HEADER: zend_long = 42;
const CURLOPT_HEADEROPT: zend_long = 229;
const CURLOPT_HSTS: zend_long = 10300;
const CURLOPT_HSTS_CTRL: zend_long = 299;
const CURLOPT_HTTP09_ALLOWED: zend_long = 285;
const CURLOPT_HTTP200ALIASES: zend_long = 10104;
const CURLOPT_HTTPAUTH: zend_long = 107;
const CURLOPT_HTTPGET: zend_long = 80;
const CURLOPT_HTTPHEADER: zend_long = 10023;
const CURLOPT_HTTPPROXYTUNNEL: zend_long = 61;
const CURLOPT_HTTP_CONTENT_DECODING: zend_long = 158;
const CURLOPT_HTTP_TRANSFER_DECODING: zend_long = 157;
const CURLOPT_HTTP_VERSION: zend_long = 84;
const CURLOPT_IGNORE_CONTENT_LENGTH: zend_long = 136;
const CURLOPT_INFILE: zend_long = 10009;
const CURLOPT_INFILESIZE: zend_long = 14;
const CURLOPT_INFILESIZE_LARGE: zend_long = 30115;
const CURLOPT_INTERFACE: zend_long = 10062;
const CURLOPT_IPRESOLVE: zend_long = 113;
const CURLOPT_ISSUERCERT: zend_long = 10170;
const CURLOPT_ISSUERCERT_BLOB: zend_long = 40295;
const CURLOPT_KEEP_SENDING_ON_ERROR: zend_long = 245;
const CURLOPT_KRBLEVEL: zend_long = 10063;
const CURLOPT_LOCALPORT: zend_long = 139;
const CURLOPT_LOCALPORTRANGE: zend_long = 140;
const CURLOPT_LOGIN_OPTIONS: zend_long = 10224;
const CURLOPT_LOW_SPEED_LIMIT: zend_long = 19;
const CURLOPT_LOW_SPEED_TIME: zend_long = 20;
const CURLOPT_MAIL_AUTH: zend_long = 10217;
const CURLOPT_MAIL_FROM: zend_long = 10186;
const CURLOPT_MAIL_RCPT: zend_long = 10187;
const CURLOPT_MAIL_RCPT_ALLLOWFAILS: zend_long = 290;
const CURLOPT_MAXAGE_CONN: zend_long = 288;
const CURLOPT_MAXCONNECTS: zend_long = 71;
const CURLOPT_MAXFILESIZE: zend_long = 114;
const CURLOPT_MAXFILESIZE_LARGE: zend_long = 30117;
const CURLOPT_MAXLIFETIME_CONN: zend_long = 314;
const CURLOPT_MAXREDIRS: zend_long = 68;
const CURLOPT_MAX_RECV_SPEED_LARGE: zend_long = 30146;
const CURLOPT_MAX_SEND_SPEED_LARGE: zend_long = 30145;
const CURLOPT_MIME_OPTIONS: zend_long = 315;
const CURLOPT_NETRC: zend_long = 51;
const CURLOPT_NETRC_FILE: zend_long = 10118;
const CURLOPT_NEW_DIRECTORY_PERMS: zend_long = 160;
const CURLOPT_NEW_FILE_PERMS: zend_long = 159;
const CURLOPT_NOBODY: zend_long = 44;
const CURLOPT_NOPROGRESS: zend_long = 43;
const CURLOPT_NOPROXY: zend_long = 10177;
const CURLOPT_NOSIGNAL: zend_long = 99;
const CURLOPT_PASSWORD: zend_long = 10174;
const CURLOPT_PATH_AS_IS: zend_long = 234;
const CURLOPT_PINNEDPUBLICKEY: zend_long = 10230;
const CURLOPT_PIPEWAIT: zend_long = 237;
const CURLOPT_PORT: zend_long = 3;
const CURLOPT_POST: zend_long = 47;
const CURLOPT_POSTFIELDS: zend_long = 10015;
const CURLOPT_POSTQUOTE: zend_long = 10039;
const CURLOPT_POSTREDIR: zend_long = 161;
const CURLOPT_PREQUOTE: zend_long = 10093;
const CURLOPT_PRE_PROXY: zend_long = 10262;
const CURLOPT_PRIVATE: zend_long = 10103;
const CURLOPT_PROTOCOLS: zend_long = 181;
const CURLOPT_PROTOCOLS_STR: zend_long = 10318;
const CURLOPT_PROXY: zend_long = 10004;
const CURLOPT_PROXYAUTH: zend_long = 111;
const CURLOPT_PROXYHEADER: zend_long = 10228;
const CURLOPT_PROXYPASSWORD: zend_long = 10176;
const CURLOPT_PROXYPORT: zend_long = 59;
const CURLOPT_PROXYTYPE: zend_long = 101;
const CURLOPT_PROXYUSERNAME: zend_long = 10175;
const CURLOPT_PROXYUSERPWD: zend_long = 10006;
const CURLOPT_PROXY_CAINFO: zend_long = 10246;
const CURLOPT_PROXY_CAINFO_BLOB: zend_long = 40310;
const CURLOPT_PROXY_CAPATH: zend_long = 10247;
const CURLOPT_PROXY_CRLFILE: zend_long = 10260;
const CURLOPT_PROXY_ISSUERCERT: zend_long = 10296;
const CURLOPT_PROXY_ISSUERCERT_BLOB: zend_long = 40297;
const CURLOPT_PROXY_KEYPASSWD: zend_long = 10258;
const CURLOPT_PROXY_PINNEDPUBLICKEY: zend_long = 10263;
const CURLOPT_PROXY_SERVICE_NAME: zend_long = 10235;
const CURLOPT_PROXY_SSLCERT: zend_long = 10254;
const CURLOPT_PROXY_SSLCERTTYPE: zend_long = 10255;
const CURLOPT_PROXY_SSLCERT_BLOB: zend_long = 40293;
const CURLOPT_PROXY_SSLKEY: zend_long = 10256;
const CURLOPT_PROXY_SSLKEYTYPE: zend_long = 10257;
const CURLOPT_PROXY_SSLKEY_BLOB: zend_long = 40294;
const CURLOPT_PROXY_SSLVERSION: zend_long = 250;
const CURLOPT_PROXY_SSL_CIPHER_LIST: zend_long = 10259;
const CURLOPT_PROXY_SSL_OPTIONS: zend_long = 261;
const CURLOPT_PROXY_SSL_VERIFYHOST: zend_long = 249;
const CURLOPT_PROXY_SSL_VERIFYPEER: zend_long = 248;
const CURLOPT_PROXY_TLS13_CIPHERS: zend_long = 10277;
const CURLOPT_PROXY_TLSAUTH_PASSWORD: zend_long = 10252;
const CURLOPT_PROXY_TLSAUTH_TYPE: zend_long = 10253;
const CURLOPT_PROXY_TLSAUTH_USERNAME: zend_long = 10251;
const CURLOPT_PROXY_TRANSFER_MODE: zend_long = 166;
const CURLOPT_PUT: zend_long = 54;
const CURLOPT_QUICK_EXIT: zend_long = 322;
const CURLOPT_QUOTE: zend_long = 10028;
const CURLOPT_RANDOM_FILE: zend_long = 10076;
const CURLOPT_RANGE: zend_long = 10007;
const CURLOPT_REDIR_PROTOCOLS: zend_long = 182;
const CURLOPT_REDIR_PROTOCOLS_STR: zend_long = 10319;
const CURLOPT_REFERER: zend_long = 10016;
const CURLOPT_REQUEST_TARGET: zend_long = 10266;
const CURLOPT_RESOLVE: zend_long = 10203;
const CURLOPT_RESUME_FROM: zend_long = 21;
const CURLOPT_RTSP_CLIENT_CSEQ: zend_long = 193;
const CURLOPT_RTSP_REQUEST: zend_long = 189;
const CURLOPT_RTSP_SERVER_CSEQ: zend_long = 194;
const CURLOPT_RTSP_SESSION_ID: zend_long = 10190;
const CURLOPT_RTSP_STREAM_URI: zend_long = 10191;
const CURLOPT_RTSP_TRANSPORT: zend_long = 10192;
const CURLOPT_SASL_AUTHZID: zend_long = 10289;
const CURLOPT_SASL_IR: zend_long = 218;
const CURLOPT_SERVER_RESPONSE_TIMEOUT: zend_long = 112;
const CURLOPT_SERVICE_NAME: zend_long = 10236;
const CURLOPT_SHARE: zend_long = 10100;
const CURLOPT_SOCKS5_AUTH: zend_long = 267;
const CURLOPT_SOCKS5_GSSAPI_NEC: zend_long = 180;
const CURLOPT_SOCKS5_GSSAPI_SERVICE: zend_long = 10179;
const CURLOPT_SSH_AUTH_TYPES: zend_long = 151;
const CURLOPT_SSH_COMPRESSION: zend_long = 268;
const CURLOPT_SSH_HOST_PUBLIC_KEY_MD5: zend_long = 10162;
const CURLOPT_SSH_HOST_PUBLIC_KEY_SHA256: zend_long = 10311;
const CURLOPT_SSH_KNOWNHOSTS: zend_long = 10183;
const CURLOPT_SSH_PRIVATE_KEYFILE: zend_long = 10153;
const CURLOPT_SSH_PUBLIC_KEYFILE: zend_long = 10152;
const CURLOPT_SSLCERT: zend_long = 10025;
const CURLOPT_SSLCERTTYPE: zend_long = 10086;
const CURLOPT_SSLCERT_BLOB: zend_long = 40291;
const CURLOPT_SSLENGINE: zend_long = 10089;
const CURLOPT_SSLENGINE_DEFAULT: zend_long = 90;
const CURLOPT_SSLKEY: zend_long = 10087;
const CURLOPT_SSLKEYPASSWD: zend_long = 10026;
const CURLOPT_SSLKEYTYPE: zend_long = 10088;
const CURLOPT_SSLKEY_BLOB: zend_long = 40292;
const CURLOPT_SSLVERSION: zend_long = 32;
const CURLOPT_SSL_CIPHER_LIST: zend_long = 10083;
const CURLOPT_SSL_EC_CURVES: zend_long = 10298;
const CURLOPT_SSL_ENABLE_ALPN: zend_long = 226;
const CURLOPT_SSL_ENABLE_NPN: zend_long = 225;
const CURLOPT_SSL_FALSESTART: zend_long = 233;
const CURLOPT_SSL_OPTIONS: zend_long = 216;
const CURLOPT_SSL_SESSIONID_CACHE: zend_long = 150;
const CURLOPT_SSL_SIGNATURE_ALGORITHMS: zend_long = 10328;
const CURLOPT_SSL_VERIFYHOST: zend_long = 81;
const CURLOPT_SSL_VERIFYPEER: zend_long = 64;
const CURLOPT_SSL_VERIFYSTATUS: zend_long = 232;
const CURLOPT_STDERR: zend_long = 10037;
const CURLOPT_STREAM_WEIGHT: zend_long = 239;
const CURLOPT_SUPPRESS_CONNECT_HEADERS: zend_long = 265;
const CURLOPT_TCP_FASTOPEN: zend_long = 244;
const CURLOPT_TCP_KEEPALIVE: zend_long = 213;
const CURLOPT_TCP_KEEPCNT: zend_long = 326;
const CURLOPT_TCP_KEEPIDLE: zend_long = 214;
const CURLOPT_TCP_KEEPINTVL: zend_long = 215;
const CURLOPT_TCP_NODELAY: zend_long = 121;
const CURLOPT_TELNETOPTIONS: zend_long = 10070;
const CURLOPT_TFTP_BLKSIZE: zend_long = 178;
const CURLOPT_TFTP_NO_OPTIONS: zend_long = 242;
const CURLOPT_TIMECONDITION: zend_long = 33;
const CURLOPT_TIMEOUT: zend_long = 13;
const CURLOPT_TIMEOUT_MS: zend_long = 155;
const CURLOPT_TIMEVALUE: zend_long = 34;
const CURLOPT_TIMEVALUE_LARGE: zend_long = 30270;
const CURLOPT_TLS13_CIPHERS: zend_long = 10276;
const CURLOPT_TLSAUTH_PASSWORD: zend_long = 10205;
const CURLOPT_TLSAUTH_TYPE: zend_long = 10206;
const CURLOPT_TLSAUTH_USERNAME: zend_long = 10204;
const CURLOPT_TRANSFERTEXT: zend_long = 53;
const CURLOPT_TRANSFER_ENCODING: zend_long = 207;
const CURLOPT_UNIX_SOCKET_PATH: zend_long = 10231;
const CURLOPT_UNRESTRICTED_AUTH: zend_long = 105;
const CURLOPT_UPKEEP_INTERVAL_MS: zend_long = 281;
const CURLOPT_UPLOAD: zend_long = 46;
const CURLOPT_UPLOAD_BUFFERSIZE: zend_long = 280;
const CURLOPT_URL: zend_long = 10002;
const CURLOPT_USERAGENT: zend_long = 10018;
const CURLOPT_USERNAME: zend_long = 10173;
const CURLOPT_USERPWD: zend_long = 10005;
const CURLOPT_USE_SSL: zend_long = 119;
const CURLOPT_VERBOSE: zend_long = 41;
const CURLOPT_WILDCARDMATCH: zend_long = 197;
const CURLOPT_WRITEHEADER: zend_long = 10029;
const CURLOPT_WS_OPTIONS: zend_long = 320;
const CURLOPT_XOAUTH2_BEARER: zend_long = 10220;

/// How an option's value is set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Long,
    /// A long, where 1 is taken as 2 with a notice.
    VerifyHost,
    /// A long, without `CURLPROTO_FILE` under `open_basedir`.
    Protocols,
    /// A long, which a thread-safe PHP refuses to turn on.
    GlobalDnsCache,
    /// A boolean, as a long.
    Flag,
    /// Only true is accepted.
    SafeUpload,
    Text,
    /// A string, without the file protocol under `open_basedir`.
    ProtocolsText,
    /// A string, or null to unset it.
    NullableText,
    Url,
    /// A path, checked against `open_basedir`.
    Path,
    /// A `curl_off_t`.
    Offset,
    /// A long, masked to `CURL_REDIR_POST_ALL`.
    PostRedirect,
    Blob,
    /// An array of strings, as a `curl_slist` the handle's clone family frees.
    List(&'static std::ffi::CStr),
    /// Whether `curl_exec` returns the body.
    ReturnTransfer,
    /// Any value, kept for `CURLINFO_PRIVATE`.
    Private,
    /// A stream resource, or null to go back to the default: the body
    /// (`CURLOPT_FILE`), the headers, the upload or libcurl's stderr. All but the
    /// upload must be writable.
    File,
    /// A `CurlShareHandle`; anything else is ignored.
    Share,
    /// The body: a string, or an array of multipart fields.
    PostFields,
    /// A callable for the body, the headers or the upload.
    Io(Stream),
    /// A callable libcurl has a callback for.
    Callback(Plain),
    /// `CURLINFO_HEADER_OUT`: whether the request header is recorded.
    HeaderOut,
    /// Kept for compatibility; nothing is set.
    Ignored,
}

/// Whether the libcurl headers the extension was built against are this version or newer.
pub(super) fn libcurl(major: u32, minor: u32, patch: u32) -> bool {
    swoole_curl_header_version >= ((major << 16) | (minor << 8) | patch)
}

/// The kind of an option, or None for one this build does not know.
fn kind(option: zend_long) -> Option<Kind> {
    match option {
        CURLINFO_HEADER_OUT => Some(Kind::HeaderOut),
        CURLOPT_SSL_VERIFYHOST => Some(Kind::VerifyHost),
        CURLOPT_PROTOCOLS | CURLOPT_REDIR_PROTOCOLS => Some(Kind::Protocols),
        // A no-op since PHP 8.4; before, a long that a thread-safe PHP refuses to turn on.
        CURLOPT_DNS_USE_GLOBAL_CACHE if PHP_VERSION_ID >= 80400 => Some(Kind::Ignored),
        CURLOPT_DNS_USE_GLOBAL_CACHE => Some(Kind::GlobalDnsCache),
        // A long since PHP 8.5; before, a boolean.
        CURLOPT_FOLLOWLOCATION if PHP_VERSION_ID >= 80500 => Some(Kind::Long),
        CURLOPT_FOLLOWLOCATION => Some(Kind::Flag),
        CURLOPT_SAFE_UPLOAD => Some(Kind::SafeUpload),
        // Nullable since PHP 8.3, so that null leaves no empty Authorization header.
        CURLOPT_USERPWD | CURLOPT_USERNAME | CURLOPT_PASSWORD if PHP_VERSION_ID < 80300 => {
            Some(Kind::Text)
        }
        CURLOPT_USERPWD | CURLOPT_USERNAME | CURLOPT_PASSWORD => Some(Kind::NullableText),
        CURLOPT_URL => Some(Kind::Url),
        CURLOPT_INFILESIZE_LARGE if PHP_VERSION_ID >= 80500 => Some(Kind::Offset),
        CURLOPT_POSTREDIR => Some(Kind::PostRedirect),
        CURLOPT_BINARYTRANSFER => Some(Kind::Ignored),
        CURLOPT_RETURNTRANSFER => Some(Kind::ReturnTransfer),
        CURLOPT_PRIVATE => Some(Kind::Private),
        CURLOPT_FILE | CURLOPT_INFILE | CURLOPT_STDERR | CURLOPT_WRITEHEADER => Some(Kind::File),
        CURLOPT_SHARE => Some(Kind::Share),
        CURLOPT_POSTFIELDS => Some(Kind::PostFields),
        CURLOPT_HTTP200ALIASES => Some(Kind::List(c"CURLOPT_HTTP200ALIASES")),
        CURLOPT_HTTPHEADER => Some(Kind::List(c"CURLOPT_HTTPHEADER")),
        CURLOPT_POSTQUOTE => Some(Kind::List(c"CURLOPT_POSTQUOTE")),
        CURLOPT_PREQUOTE => Some(Kind::List(c"CURLOPT_PREQUOTE")),
        CURLOPT_QUOTE => Some(Kind::List(c"CURLOPT_QUOTE")),
        CURLOPT_TELNETOPTIONS => Some(Kind::List(c"CURLOPT_TELNETOPTIONS")),
        CURLOPT_MAIL_RCPT => Some(Kind::List(c"CURLOPT_MAIL_RCPT")),
        CURLOPT_RESOLVE => Some(Kind::List(c"CURLOPT_RESOLVE")),
        CURLOPT_PROXYHEADER => Some(Kind::List(c"CURLOPT_PROXYHEADER")),
        CURLOPT_CONNECT_TO => Some(Kind::List(c"CURLOPT_CONNECT_TO")),
        _ => long(option)
            .or_else(|| text(option))
            .or_else(|| newer_long(option))
            .or_else(|| newer(option))
            .or_else(|| Stream::of(option).map(Kind::Io))
            .or_else(|| Plain::of(option).map(Kind::Callback)),
    }
}

/// The longs every supported libcurl and PHP have.
const fn long(option: zend_long) -> Option<Kind> {
    match option {
        CURLOPT_AUTOREFERER
        | CURLOPT_BUFFERSIZE
        | CURLOPT_CONNECTTIMEOUT
        | CURLOPT_COOKIESESSION
        | CURLOPT_CRLF
        | CURLOPT_DNS_CACHE_TIMEOUT
        | CURLOPT_FAILONERROR
        | CURLOPT_FILETIME
        | CURLOPT_FORBID_REUSE
        | CURLOPT_FRESH_CONNECT
        | CURLOPT_FTP_USE_EPRT
        | CURLOPT_FTP_USE_EPSV
        | CURLOPT_HEADER
        | CURLOPT_HTTPGET
        | CURLOPT_HTTPPROXYTUNNEL
        | CURLOPT_HTTP_VERSION
        | CURLOPT_INFILESIZE
        | CURLOPT_LOW_SPEED_LIMIT
        | CURLOPT_LOW_SPEED_TIME
        | CURLOPT_MAXCONNECTS
        | CURLOPT_MAXREDIRS
        | CURLOPT_NETRC
        | CURLOPT_NOBODY
        | CURLOPT_NOPROGRESS
        | CURLOPT_NOSIGNAL
        | CURLOPT_PORT
        | CURLOPT_POST
        | CURLOPT_PROXYPORT
        | CURLOPT_PROXYTYPE
        | CURLOPT_PUT
        | CURLOPT_RESUME_FROM
        | CURLOPT_SSLVERSION
        | CURLOPT_SSL_VERIFYPEER
        | CURLOPT_TIMECONDITION
        | CURLOPT_TIMEOUT
        | CURLOPT_TIMEVALUE
        | CURLOPT_TRANSFERTEXT
        | CURLOPT_UNRESTRICTED_AUTH
        | CURLOPT_UPLOAD
        | CURLOPT_VERBOSE
        | CURLOPT_HTTPAUTH
        | CURLOPT_FTP_CREATE_MISSING_DIRS
        | CURLOPT_PROXYAUTH
        | CURLOPT_SERVER_RESPONSE_TIMEOUT
        | CURLOPT_IPRESOLVE
        | CURLOPT_MAXFILESIZE
        | CURLOPT_TCP_NODELAY
        | CURLOPT_FTPSSLAUTH
        | CURLOPT_IGNORE_CONTENT_LENGTH
        | CURLOPT_FTP_SKIP_PASV_IP
        | CURLOPT_FTP_FILEMETHOD
        | CURLOPT_CONNECT_ONLY
        | CURLOPT_LOCALPORT
        | CURLOPT_LOCALPORTRANGE
        | CURLOPT_SSL_SESSIONID_CACHE
        | CURLOPT_FTP_SSL_CCC
        | CURLOPT_SSH_AUTH_TYPES
        | CURLOPT_CONNECTTIMEOUT_MS
        | CURLOPT_HTTP_CONTENT_DECODING
        | CURLOPT_HTTP_TRANSFER_DECODING
        | CURLOPT_TIMEOUT_MS
        | CURLOPT_NEW_DIRECTORY_PERMS
        | CURLOPT_NEW_FILE_PERMS
        | CURLOPT_USE_SSL
        | CURLOPT_APPEND
        | CURLOPT_DIRLISTONLY
        | CURLOPT_PROXY_TRANSFER_MODE
        | CURLOPT_ADDRESS_SCOPE
        | CURLOPT_CERTINFO
        | CURLOPT_SOCKS5_GSSAPI_NEC
        | CURLOPT_TFTP_BLKSIZE
        | CURLOPT_FTP_USE_PRET
        | CURLOPT_RTSP_CLIENT_CSEQ
        | CURLOPT_RTSP_REQUEST
        | CURLOPT_RTSP_SERVER_CSEQ
        | CURLOPT_WILDCARDMATCH
        | CURLOPT_GSSAPI_DELEGATION
        | CURLOPT_ACCEPTTIMEOUT_MS
        | CURLOPT_SSL_OPTIONS
        | CURLOPT_TCP_KEEPALIVE
        | CURLOPT_TCP_KEEPIDLE
        | CURLOPT_TCP_KEEPINTVL
        | CURLOPT_SASL_IR
        | CURLOPT_EXPECT_100_TIMEOUT_MS
        | CURLOPT_SSL_ENABLE_ALPN
        | CURLOPT_SSL_ENABLE_NPN
        | CURLOPT_HEADEROPT
        | CURLOPT_SSL_VERIFYSTATUS
        | CURLOPT_PATH_AS_IS
        | CURLOPT_SSL_FALSESTART
        | CURLOPT_PIPEWAIT
        | CURLOPT_STREAM_WEIGHT
        | CURLOPT_TFTP_NO_OPTIONS
        | CURLOPT_TCP_FASTOPEN
        | CURLOPT_KEEP_SENDING_ON_ERROR
        | CURLOPT_PROXY_SSL_OPTIONS
        | CURLOPT_PROXY_SSL_VERIFYHOST
        | CURLOPT_PROXY_SSL_VERIFYPEER
        | CURLOPT_PROXY_SSLVERSION
        | CURLOPT_SUPPRESS_CONNECT_HEADERS
        | CURLOPT_SOCKS5_AUTH
        | CURLOPT_SSH_COMPRESSION => Some(Kind::Long),
        _ => None,
    }
}

/// The strings, nullable strings, offsets and paths every supported libcurl and PHP have.
const fn text(option: zend_long) -> Option<Kind> {
    match option {
        CURLOPT_CAINFO
        | CURLOPT_CAPATH
        | CURLOPT_COOKIE
        | CURLOPT_EGDSOCKET
        | CURLOPT_INTERFACE
        | CURLOPT_PROXY
        | CURLOPT_PROXYUSERPWD
        | CURLOPT_REFERER
        | CURLOPT_SSLCERTTYPE
        | CURLOPT_SSLENGINE
        | CURLOPT_SSLENGINE_DEFAULT
        | CURLOPT_SSLKEY
        | CURLOPT_SSLKEYPASSWD
        | CURLOPT_SSLKEYTYPE
        | CURLOPT_SSL_CIPHER_LIST
        | CURLOPT_USERAGENT
        | CURLOPT_COOKIELIST
        | CURLOPT_FTP_ALTERNATIVE_TO_USER
        | CURLOPT_SSH_HOST_PUBLIC_KEY_MD5
        | CURLOPT_PROXYPASSWORD
        | CURLOPT_PROXYUSERNAME
        | CURLOPT_NOPROXY
        | CURLOPT_SOCKS5_GSSAPI_SERVICE
        | CURLOPT_MAIL_FROM
        | CURLOPT_RTSP_STREAM_URI
        | CURLOPT_RTSP_TRANSPORT
        | CURLOPT_TLSAUTH_TYPE
        | CURLOPT_TLSAUTH_PASSWORD
        | CURLOPT_TLSAUTH_USERNAME
        | CURLOPT_TRANSFER_ENCODING
        | CURLOPT_DNS_SERVERS
        | CURLOPT_MAIL_AUTH
        | CURLOPT_LOGIN_OPTIONS
        | CURLOPT_PINNEDPUBLICKEY
        | CURLOPT_PROXY_SERVICE_NAME
        | CURLOPT_SERVICE_NAME
        | CURLOPT_DEFAULT_PROTOCOL
        | CURLOPT_PRE_PROXY
        | CURLOPT_PROXY_CAINFO
        | CURLOPT_PROXY_CAPATH
        | CURLOPT_PROXY_CRLFILE
        | CURLOPT_PROXY_KEYPASSWD
        | CURLOPT_PROXY_PINNEDPUBLICKEY
        | CURLOPT_PROXY_SSL_CIPHER_LIST
        | CURLOPT_PROXY_SSLCERT
        | CURLOPT_PROXY_SSLCERTTYPE
        | CURLOPT_PROXY_SSLKEY
        | CURLOPT_PROXY_SSLKEYTYPE
        | CURLOPT_PROXY_TLSAUTH_PASSWORD
        | CURLOPT_PROXY_TLSAUTH_TYPE
        | CURLOPT_PROXY_TLSAUTH_USERNAME
        | CURLOPT_ABSTRACT_UNIX_SOCKET
        | CURLOPT_REQUEST_TARGET => Some(Kind::Text),
        CURLOPT_CUSTOMREQUEST
        | CURLOPT_FTPPORT
        | CURLOPT_RANGE
        | CURLOPT_FTP_ACCOUNT
        | CURLOPT_RTSP_SESSION_ID
        | CURLOPT_ACCEPT_ENCODING
        | CURLOPT_DNS_INTERFACE
        | CURLOPT_DNS_LOCAL_IP4
        | CURLOPT_DNS_LOCAL_IP6
        | CURLOPT_XOAUTH2_BEARER
        | CURLOPT_UNIX_SOCKET_PATH
        | CURLOPT_KRBLEVEL => Some(Kind::NullableText),
        CURLOPT_MAX_RECV_SPEED_LARGE | CURLOPT_MAX_SEND_SPEED_LARGE | CURLOPT_MAXFILESIZE_LARGE => {
            Some(Kind::Offset)
        }
        CURLOPT_COOKIEFILE
        | CURLOPT_COOKIEJAR
        | CURLOPT_RANDOM_FILE
        | CURLOPT_SSLCERT
        | CURLOPT_NETRC_FILE
        | CURLOPT_SSH_PRIVATE_KEYFILE
        | CURLOPT_SSH_PUBLIC_KEYFILE
        | CURLOPT_CRLFILE
        | CURLOPT_ISSUERCERT
        | CURLOPT_SSH_KNOWNHOSTS => Some(Kind::Path),
        _ => None,
    }
}

/// The longs a newer libcurl or PHP added, as the copies' `#if`s gate them.
fn newer_long(option: zend_long) -> Option<Kind> {
    match option {
        CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS if libcurl(7, 59, 0) => Some(Kind::Long),
        CURLOPT_DNS_SHUFFLE_ADDRESSES | CURLOPT_HAPROXYPROTOCOL if libcurl(7, 60, 0) => {
            Some(Kind::Long)
        }
        CURLOPT_DISALLOW_USERNAME_IN_URL if libcurl(7, 61, 0) => Some(Kind::Long),
        CURLOPT_UPKEEP_INTERVAL_MS | CURLOPT_UPLOAD_BUFFERSIZE if libcurl(7, 62, 0) => {
            Some(Kind::Long)
        }
        CURLOPT_HTTP09_ALLOWED if libcurl(7, 64, 0) => Some(Kind::Long),
        CURLOPT_ALTSVC_CTRL if libcurl(7, 64, 1) => Some(Kind::Long),
        CURLOPT_MAXAGE_CONN if libcurl(7, 65, 0) => Some(Kind::Long),
        CURLOPT_MAIL_RCPT_ALLLOWFAILS if libcurl(7, 69, 0) => Some(Kind::Long),
        CURLOPT_HSTS_CTRL if libcurl(7, 74, 0) => Some(Kind::Long),
        CURLOPT_DOH_SSL_VERIFYHOST | CURLOPT_DOH_SSL_VERIFYPEER | CURLOPT_DOH_SSL_VERIFYSTATUS
            if libcurl(7, 76, 0) =>
        {
            Some(Kind::Long)
        }
        CURLOPT_MAXLIFETIME_CONN if libcurl(7, 80, 0) => Some(Kind::Long),
        CURLOPT_MIME_OPTIONS if libcurl(7, 81, 0) && PHP_VERSION_ID >= 80300 => Some(Kind::Long),
        CURLOPT_WS_OPTIONS if libcurl(7, 86, 0) && PHP_VERSION_ID >= 80300 => Some(Kind::Long),
        CURLOPT_CA_CACHE_TIMEOUT | CURLOPT_QUICK_EXIT
            if libcurl(7, 87, 0) && PHP_VERSION_ID >= 80300 =>
        {
            Some(Kind::Long)
        }
        CURLOPT_TCP_KEEPCNT if libcurl(8, 9, 0) && PHP_VERSION_ID >= 80400 => Some(Kind::Long),
        _ => None,
    }
}

/// The other options a newer libcurl or PHP added, as [`newer_long`].
fn newer(option: zend_long) -> Option<Kind> {
    match option {
        CURLOPT_PROXY_TLS13_CIPHERS | CURLOPT_TLS13_CIPHERS if libcurl(7, 61, 0) => {
            Some(Kind::Text)
        }
        CURLOPT_ALTSVC if libcurl(7, 64, 1) => Some(Kind::Text),
        CURLOPT_SASL_AUTHZID if libcurl(7, 66, 0) => Some(Kind::Text),
        CURLOPT_PROXY_ISSUERCERT if libcurl(7, 71, 0) => Some(Kind::Text),
        CURLOPT_SSL_EC_CURVES if libcurl(7, 73, 0) => Some(Kind::Text),
        CURLOPT_AWS_SIGV4 if libcurl(7, 75, 0) => Some(Kind::Text),
        CURLOPT_SSH_HOST_PUBLIC_KEY_SHA256 if libcurl(7, 80, 0) => Some(Kind::Text),
        CURLOPT_PROTOCOLS_STR | CURLOPT_REDIR_PROTOCOLS_STR
            if libcurl(7, 85, 0) && PHP_VERSION_ID >= 80300 =>
        {
            Some(Kind::ProtocolsText)
        }
        CURLOPT_DOH_URL if libcurl(7, 62, 0) => Some(Kind::NullableText),
        CURLOPT_HSTS if libcurl(7, 74, 0) => Some(Kind::NullableText),
        CURLOPT_SSL_SIGNATURE_ALGORITHMS if libcurl(8, 14, 0) && PHP_VERSION_ID >= 80500 => {
            Some(Kind::NullableText)
        }
        CURLOPT_TIMEVALUE_LARGE if libcurl(7, 59, 0) => Some(Kind::Offset),
        CURLOPT_ISSUERCERT_BLOB
        | CURLOPT_PROXY_ISSUERCERT_BLOB
        | CURLOPT_PROXY_SSLCERT_BLOB
        | CURLOPT_PROXY_SSLKEY_BLOB
        | CURLOPT_SSLCERT_BLOB
        | CURLOPT_SSLKEY_BLOB
            if libcurl(7, 71, 0) =>
        {
            Some(Kind::Blob)
        }
        CURLOPT_CAINFO_BLOB | CURLOPT_PROXY_CAINFO_BLOB if libcurl(7, 77, 0) => Some(Kind::Blob),
        _ => None,
    }
}

fn setopt(call: &Call<'_>) {
    let Some(handle) = argument(call, 3) else {
        return;
    };
    let Ok(option) = call.long(2, 0) else { return };
    let Some(value) = call.value(3) else { return };
    call.set_bool(set(call, handle, option, value.referred(), false));
}

fn setopt_array(call: &Call<'_>) {
    let Some(handle) = argument(call, 2) else {
        return;
    };
    let Ok(options) = call.array(2) else { return };
    // An option's conversion can run PHP code, which must not free the table.
    let Some(held) = options.hold() else { return };
    let Some(table) = held.array() else { return };
    let mut entries = table.entries();
    while let Some((key, value)) = entries.next_keyed() {
        let Key::Index(option) = key else {
            call.argument_value_error(2, c"contains an invalid cURL option");
            return;
        };
        if !set(
            call,
            handle,
            zend_long::from_ne_bytes(option.to_ne_bytes()),
            value.referred(),
            true,
        ) {
            call.set_bool(false);
            return;
        }
    }
    call.set_bool(true);
}

/// `_php_curl_setopt`: whether the option was set.
fn set(
    call: &Call<'_>,
    handle: PhpHandle<'_>,
    option: zend_long,
    value: &Value,
    array: bool,
) -> bool {
    let Some(kind) = kind(option) else {
        call.argument_value_error(
            2,
            if array {
                c"must contain only valid cURL options"
            } else {
                c"is not a valid cURL option"
            },
        );
        swoole_curl_save_error(handle, UNKNOWN_OPTION);
        return false;
    };
    // Every option here is a curl.h value, which fits a C int.
    let id = c_int::try_from(option).unwrap_or_default();
    match kind {
        Kind::Long
        | Kind::VerifyHost
        | Kind::Protocols
        | Kind::GlobalDnsCache
        | Kind::PostRedirect => {
            let Some(mut long) = value.applied_long() else {
                return false;
            };
            match kind {
                Kind::VerifyHost if long == 1 => {
                    if !warn(
                        E_NOTICE,
                        c"CURLOPT_SSL_VERIFYHOST no longer accepts the value 1, value 2 will be used instead",
                    ) {
                        return false;
                    }
                    long = 2;
                }
                Kind::Protocols if long & CURLPROTO_FILE != 0 && swoole_curl_open_basedir() => {
                    warn(
                        E_WARNING,
                        c"CURLPROTO_FILE cannot be activated when an open_basedir is set",
                    );
                    return false;
                }
                Kind::GlobalDnsCache if swoole_rs_zend::ZTS && long != 0 => {
                    warn(
                        E_WARNING,
                        c"CURLOPT_DNS_USE_GLOBAL_CACHE cannot be activated when thread safety is enabled",
                    );
                    return false;
                }
                Kind::PostRedirect => long &= CURL_REDIR_POST_ALL,
                _ => {}
            }
            set_long(handle, id, long)
        }
        Kind::Flag => set_long(handle, id, value.truthy().into()),
        Kind::SafeUpload => {
            if !value.truthy() {
                value_error(call, "Disabling safe uploads is no longer supported");
                return false;
            }
            swoole_curl_save_error(handle, 0);
            true
        }
        Kind::Offset => {
            let Some(long) = value.applied_long() else {
                return false;
            };
            swoole_curl_setopt_off_t(handle, id, long) == 0
        }
        Kind::NullableText if value.is_null() => swoole_curl_setopt_string(handle, id, None) == 0,
        Kind::Text
        | Kind::ProtocolsText
        | Kind::NullableText
        | Kind::Url
        | Kind::Path
        | Kind::Blob => {
            let Some(text) = value.applied_string() else {
                return false;
            };
            match kind {
                Kind::ProtocolsText
                    if swoole_curl_open_basedir()
                        && [&b"file"[..], b"all"]
                            .iter()
                            .any(|protocol| contains_ignoring_case(text.bytes(), protocol)) =>
                {
                    warn(
                        E_WARNING,
                        c"The FILE protocol cannot be activated when an open_basedir is set",
                    );
                    false
                }
                Kind::Url => {
                    let mut set = false;
                    bailout::defer(swoole_rs_zend::contain_with(|| {
                        set = swoole_curl_set_url(handle, &text);
                    }));
                    set
                }
                Kind::Blob => swoole_curl_setopt_blob(handle, id, &text) == 0,
                _ => {
                    if kind == Kind::Path && !text.bytes().is_empty() {
                        let mut allowed = false;
                        bailout::defer(swoole_rs_zend::contain_with(|| {
                            allowed = swoole_curl_allowed_path(&text);
                        }));
                        if !allowed {
                            return false;
                        }
                    }
                    if text.bytes().contains(&0) {
                        value_error(call, "cURL option must not contain any null bytes");
                        return false;
                    }
                    swoole_curl_setopt_string(handle, id, Some(&text)) == 0
                }
            }
        }
        Kind::List(name) => set_list(call, handle, id, name, value),
        Kind::ReturnTransfer => {
            let method = if value.truthy() {
                PHP_CURL_RETURN
            } else {
                PHP_CURL_STDOUT
            };
            swoole_curl_set_write_method(handle, method);
            swoole_curl_save_error(handle, 0);
            true
        }
        Kind::Private => {
            // The previous value is released once the slot is no longer borrowed: its
            // destructor can call back into this handle.
            let previous = std::mem::replace(swoole_curl_private(handle), value.copy());
            drop(previous);
            true
        }
        Kind::File => set_file(call, handle, option, value),
        Kind::PostFields => super::postfields::set(handle, value),
        Kind::Io(stream) => callbacks::set_io(call, handle, stream, value, array),
        Kind::Callback(which) => callbacks::set_plain(call, handle, which, value, array),
        Kind::HeaderOut => callbacks::set_header_out(handle, value.truthy()),
        Kind::Share => {
            swoole_curl_set_share(handle, value);
            swoole_curl_save_error(handle, 0);
            true
        }
        Kind::Ignored => {
            swoole_curl_save_error(handle, 0);
            true
        }
    }
}

/// A stream's `FILE *`, which the stream owns.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(super) struct StdioFile(std::ptr::NonNull<std::ffi::c_void>);

/// The file-handle options. The handle keeps a reference to the stream it writes
/// to or reads from.
fn set_file(call: &Call<'_>, handle: PhpHandle<'_>, option: zend_long, value: &Value) -> bool {
    let id = c_int::try_from(option).unwrap_or_default();
    let file = if value.is_null() {
        None
    } else {
        let (mut file, mut writable) = (None, false);
        // Fetching the resource throws for anything but a stream, and the cast can warn.
        bailout::defer(swoole_rs_zend::contain_with(|| {
            file = swoole_curl_stream_file(value, &mut writable);
        }));
        let Some(file) = file else { return false };
        if option != CURLOPT_INFILE && !writable {
            value_error(call, "The provided file handle must be writable");
            return false;
        }
        Some(file)
    };
    let kept = if file.is_some() {
        value.copy()
    } else {
        swoole_rs_zend::Owned::default()
    };
    // The stream the handle kept before is released once the slot is no longer
    // borrowed and the handle no longer uses its file.
    let previous = std::mem::replace(swoole_curl_file_stream(handle, id), kept);
    let set = swoole_curl_set_file(handle, id, file) == 0;
    drop(previous);
    set
}

/// A `curl_slist` this side owns: one being built, freed unless [`swoole_curl_setopt_list`]
/// takes it, or one `curl_easy_getinfo` allocated.
#[repr(transparent)]
pub(super) struct List(pub(super) Option<std::ptr::NonNull<std::ffi::c_void>>);

impl Drop for List {
    fn drop(&mut self) {
        if self.0.is_some() {
            swoole_curl_list_free(Self(self.0.take()));
        }
    }
}

/// The list options: each element as a string, in order.
fn set_list(
    call: &Call<'_>,
    handle: PhpHandle<'_>,
    id: c_int,
    name: &std::ffi::CStr,
    value: &Value,
) -> bool {
    // A conversion can run PHP code, which must not free the table.
    let Some(held) = value.hold() else {
        let function = call
            .function()
            .name()
            .map_or(&[][..], |function| function.bytes());
        let message = [
            function,
            b"(): The ",
            name.to_bytes(),
            b" option must have an array value",
        ]
        .concat();
        swoole_rs_zend::type_error(&CString::new(message).unwrap_or_default());
        return false;
    };
    let Some(table) = held.array() else {
        return false;
    };
    let mut list = List(None);
    let mut entries = table.entries();
    while let Some((_, element)) = entries.next_keyed() {
        let Some(text) = element.referred().applied_string() else {
            return false;
        };
        if !swoole_curl_list_append(&mut list, &text) {
            warn(E_WARNING, c"Could not build curl_slist");
            return false;
        }
    }
    swoole_curl_setopt_list(handle, id, list) == 0
}

fn set_long(handle: PhpHandle<'_>, id: c_int, long: zend_long) -> bool {
    // A zend_long is a C long on every platform PHP's Linux builds run.
    swoole_curl_setopt_long(handle, id, long) == 0
}

/// `php_error_docref(NULL, level, message)`: false once it bailed out.
pub(super) fn warn(level: i32, message: &std::ffi::CStr) -> bool {
    let reported = swoole_rs_zend::report(level, message);
    let ok = reported.is_ok();
    bailout::defer(reported);
    ok
}

/// `zend_value_error("%s(): message", get_active_function_name())`.
fn value_error(call: &Call<'_>, message: &str) {
    let name = call.function().name().map_or(&[][..], |name| name.bytes());
    let text = [name, b"(): ", message.as_bytes()].concat();
    swoole_rs_zend::value_error(&CString::new(text).unwrap_or_default());
}

/// `php_memnistr`: whether `needle` is in `haystack`, ASCII case ignored.
fn contains_ignoring_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}
