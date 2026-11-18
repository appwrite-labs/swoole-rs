//! `cfg(zts)` when the PHP being built for is thread-safe, so per-thread state
//! is thread-local there and plain process state otherwise, as Swoole's
//! `SW_THREAD_LOCAL` is. Shared by the crates that keep such state (their
//! `build` key points here), under their `php` feature or when building corebase.
//! A crate's `thread` feature is upstream's `SW_THREAD` build without PHP.
//! `cfg(php85)` when it is PHP 8.5 or later, whose `pdo_dbh_t` holds its flags in
//! bytes rather than bit-fields.

pub(crate) fn main() {
    println!("cargo::rustc-check-cfg=cfg(zts)");
    println!("cargo::rustc-check-cfg=cfg(php85)");
    if std::env::var_os("CARGO_FEATURE_THREAD").is_some() {
        println!("cargo::rustc-cfg=zts");
        return;
    }
    println!("cargo::rerun-if-env-changed=PHP_CONFIG");
    if std::env::var_os("CARGO_FEATURE_PHP").is_none()
        && std::env::var("CARGO_PKG_NAME").as_deref() != Ok("swoole-rs-corebase")
    {
        return;
    }
    let php_config = std::env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".into());
    let Ok(output) = std::process::Command::new(&php_config)
        .arg("--include-dir")
        .output()
    else {
        return;
    };
    let include = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let version = include.join("main/php_version.h");
    println!("cargo::rerun-if-changed={}", version.display());
    let id = std::fs::read_to_string(&version).ok().and_then(|text| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix("#define PHP_VERSION_ID"))
            .and_then(|id| id.trim().parse::<u32>().ok())
    });
    if id.is_some_and(|id| id >= 80500) {
        println!("cargo::rustc-cfg=php85");
    }
    let header = include.join("main/php_config.h");
    println!("cargo::rerun-if-changed={}", header.display());
    let zts = std::fs::read_to_string(&header)
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "#define ZTS 1"));
    if zts {
        println!("cargo::rustc-cfg=zts");
    }
}
