//! Compile src/stdext.c, the crate's C remainder (README.md), when the crate
//! is live: the `php` and `active` (native `SW_STDEXT`) features.

fn main() {
    #[cfg(all(feature = "php", feature = "active"))]
    native::compile();
}

#[cfg(all(feature = "php", feature = "active"))]
mod native {
    use std::{env, path::PathBuf, process::Command};

    /// The `-I` directories a command prints.
    fn include_flags(command: &mut Command) -> Vec<PathBuf> {
        command
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .filter_map(|flag| flag.strip_prefix("-I"))
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn compile() {
        println!("cargo::rerun-if-env-changed=PHP_CONFIG");
        println!("cargo::rerun-if-changed=src/stdext.c");
        // The pinned upstream tree holds Swoole's copy of zend_execute.c's
        // private primitives. The rust tier has no tree and links nothing.
        let upstream = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/upstream/src");
        let primitives = upstream.join("thirdparty/php/zend/zend_execute.c");
        println!("cargo::rerun-if-changed={}", primitives.display());
        if !primitives.is_file() {
            println!(
                "cargo::warning=phpstdext: no {} (tools/build_upstream.py fetches it): stdext's Zend calls stay unlinked",
                primitives.display()
            );
            return;
        }
        let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".into());
        cc::Build::new()
            .file("src/stdext.c")
            .includes(include_flags(Command::new(php_config).arg("--includes")))
            // ext/pcre's header includes PCRE2's when PHP links a system PCRE2.
            .includes(include_flags(
                Command::new("pkg-config").args(["--cflags", "libpcre2-8"]),
            ))
            .include(upstream)
            // Swoole's copy is upstream code, with helpers these wrappers never call.
            .warnings(false)
            .compile("swoole_rs_phpstdext");
    }
}
