//! `cfg(zts)` when the PHP being built for is thread-safe, so per-thread state
//! is thread-local there and plain process state otherwise, as Swoole's
//! `SW_THREAD_LOCAL` is.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(zts)");
    println!("cargo::rerun-if-env-changed=PHP_CONFIG");
    if std::env::var_os("CARGO_FEATURE_PHP").is_none() {
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
