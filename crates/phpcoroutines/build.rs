//! `cfg(zts)` when the PHP being built for is thread-safe, so per-thread state
//! is thread-local there and plain process state otherwise, as Swoole's
//! `SW_THREAD_LOCAL` is. Shared by the crates that keep such state (their
//! `build` key points here), under their `php` or `upstream-symbols` feature.
//! A crate's `thread` feature is upstream's `SW_THREAD` build without PHP.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(zts)");
    if std::env::var_os("CARGO_FEATURE_THREAD").is_some() {
        println!("cargo::rustc-cfg=zts");
        return;
    }
    println!("cargo::rerun-if-env-changed=PHP_CONFIG");
    if std::env::var_os("CARGO_FEATURE_PHP").is_none()
        && std::env::var_os("CARGO_FEATURE_UPSTREAM_SYMBOLS").is_none()
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
    let header = std::path::Path::new(String::from_utf8_lossy(&output.stdout).trim())
        .join("main/php_config.h");
    println!("cargo::rerun-if-changed={}", header.display());
    let zts = std::fs::read_to_string(&header)
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "#define ZTS 1"));
    if zts {
        println!("cargo::rustc-cfg=zts");
    }
}
