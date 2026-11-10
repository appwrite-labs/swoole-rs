//! Compile what the extension still needs of C and C++ (native/ and the bundled
//! thirdparty sources config.m4 lists) and link it into `swoole.so`, in place of
//! phpize and configure. The thirdparty sources are native/swoole-src's, except
//! llhttp, which is still the pin's (`SWOOLE_SRC`).

fn main() {
    println!("cargo::rerun-if-env-changed=PHP_CONFIG");
    println!("cargo::rerun-if-env-changed=SWOOLE_SRC");
    // Zend's symbols resolve against the PHP that loads the extension.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos") {
        println!("cargo::rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
    #[cfg(feature = "php")]
    php::build();
}

#[cfg(feature = "php")]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a build script reports failure by panicking"
)]
mod php {
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process::Command,
    };

    /// What configure's checks define in config.h that native/ reads, by target
    /// OS.
    const LINUX: &[&str] = &[
        "HAVE_AI_ALL",
        "HAVE_AI_V4MAPPED",
        "HAVE_CPU_AFFINITY",
        "HAVE_EPOLL",
        "HAVE_EVENTFD",
        "HAVE_EXECINFO",
        "HAVE_FUTEX",
        "HAVE_MALLOC_TRIM",
        "HAVE_MSGQUEUE",
        "HAVE_MUTEX_TIMEDLOCK",
        "HAVE_PTHREAD_BARRIER",
        "HAVE_REUSEPORT",
        "HAVE_RWLOCK",
        "HAVE_RWLOCK_TIMEDRDLOCK",
        "HAVE_SIGNALFD",
        "HAVE_SPINLOCK",
    ];
    const MACOS: &[&str] = &[
        "HAVE_AI_ALL",
        "HAVE_AI_V4MAPPED",
        "HAVE_EXECINFO",
        "HAVE_KQUEUE",
        "HAVE_MSGQUEUE",
        "HAVE_REUSEPORT",
        "HAVE_RWLOCK",
    ];
    /// Every build's: the switches config.m4 sets unconditionally, less
    /// `COMPILE_DL_SWOOLE` (lib.rs exports `get_module()`).
    const COMMON: &[&str] = &[
        "SWOOLE_RS_BRIDGE",
        "SW_HAVE_BROTLI",
        "SW_HAVE_ZLIB",
        "SW_HAVE_ZSTD",
    ];

    /// The build variants' features and what config.m4 defines for each.
    const VARIANTS: &[(&str, &[&str])] = &[
        ("stdext", &["SW_STDEXT"]),
        ("iouring", &["SW_USE_IOURING"]),
        ("uring_socket", &["SW_USE_URING_SOCKET"]),
        ("cares", &["SW_USE_CARES", "HAVE_CARES"]),
        ("thread_context", &["SW_USE_THREAD_CONTEXT"]),
        ("thread", &["SW_THREAD"]),
    ];

    /// What config.m4 adds for each switch of the merge tier's PHPT profile whose
    /// `crates/ext` feature is on (`tools/build_ext.py` maps the switches onto them).
    #[derive(Default)]
    struct Profile {
        defines: Vec<String>,
        c: Vec<PathBuf>,
        cxx: Vec<PathBuf>,
        includes: Vec<PathBuf>,
        libs: Vec<&'static str>,
    }

    impl Profile {
        fn define(&mut self, names: &[&str]) {
            self.defines
                .extend(names.iter().map(|name| format!("{name} 1")));
        }
    }

    fn feature(name: &str) -> bool {
        env::var_os(format!("CARGO_FEATURE_{}", name.to_uppercase())).is_some()
    }

    fn pkg_includes(module: &str) -> Vec<PathBuf> {
        include_flags(Command::new("pkg-config").args(["--cflags", module])).unwrap_or_default()
    }

    /// config.m4's `SW_PHP_VERSION_ID`: 84 for PHP 8.4.x.
    fn php_version_id(php_config: &str) -> u32 {
        let output = Command::new(php_config)
            .arg("--version")
            .output()
            .expect("ext: php-config --version ($PHP_CONFIG) failed");
        let version = String::from_utf8_lossy(&output.stdout);
        let mut parts = version
            .trim()
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        parts.next().unwrap_or(0) * 10 + parts.next().unwrap_or(0)
    }

    fn profile(third: &Path, php: u32) -> Profile {
        let mut p = Profile::default();
        let bundled = third.join(format!("php{php}"));
        if feature("sockets") {
            p.define(&["SW_SOCKETS", "HAVE_SOCKETS"]);
        }
        for (_, defines) in VARIANTS.iter().filter(|(name, _)| feature(name)) {
            p.define(defines);
        }
        if feature("mysqlnd") {
            p.define(&["SW_USE_MYSQLND"]);
        }
        if feature("curl") {
            p.define(&["SW_USE_CURL"]);
            let dir = third.join(if php >= 84 { "php84/curl" } else { "php/curl" });
            p.cxx
                .extend(["interface.cc", "multi.cc"].map(|f| dir.join(f)));
            p.includes.extend(pkg_includes("libcurl"));
            p.libs.push("curl");
        }
        if feature("pgsql") {
            p.define(&["SW_USE_PGSQL"]);
            // config.m4 probes for PQsendClosePrepared, which libpq 17 added.
            let major = Command::new("pkg-config")
                .args(["--modversion", "libpq"])
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split('.')
                        .next()?
                        .trim()
                        .parse::<u32>()
                        .ok()
                });
            if major.is_some_and(|major| major >= 17) {
                p.define(&["HAVE_PQCLOSEPREPARED"]);
            }
            let dir = bundled.join("pdo_pgsql");
            p.c.extend(["pgsql_driver.c", "pgsql_statement.c"].map(|f| dir.join(f)));
            if php >= 84 {
                p.c.push(dir.join("pgsql_sql_parser.c"));
            }
            p.includes.extend(pkg_includes("libpq"));
            p.libs.push("pq");
        }
        if feature("odbc") {
            // `--with-swoole-odbc=unixODBC,DIR`: SWOOLE_ODBC_DIR is DIR.
            println!("cargo::rerun-if-env-changed=SWOOLE_ODBC_DIR");
            let prefix =
                PathBuf::from(env::var("SWOOLE_ODBC_DIR").unwrap_or_else(|_| "/usr".into()));
            let include = prefix.join("include");
            p.define(&["SW_USE_ODBC"]);
            p.defines.push(r#"PDO_ODBC_TYPE "unixODBC""#.into());
            for header in [
                "odbc.h",
                "odbcsdk.h",
                "iodbc.h",
                "sqlunix.h",
                "sqltypes.h",
                "sqlucode.h",
                "sql.h",
                "isql.h",
                "sqlext.h",
                "isqlext.h",
                "udbcext.h",
                "sqlcli1.h",
                "LibraryManager.h",
                "cli0core.h",
                "cli0ext.h",
                "cli0cli.h",
                "cli0defs.h",
                "cli0env.h",
            ] {
                if include.join(header).is_file() {
                    p.defines.push(format!(
                        "HAVE_{} 1",
                        header.to_uppercase().replace('.', "_")
                    ));
                }
            }
            let dir = bundled.join("pdo_odbc");
            p.c.extend(["odbc_driver.c", "odbc_stmt.c"].map(|f| dir.join(f)));
            p.includes.push(include);
            p.libs.push("odbc");
        }
        if feature("oci") {
            // `--with-swoole-oracle=instantclient,DIR`, from the zip files: SWOOLE_ORACLE_DIR is DIR.
            println!("cargo::rerun-if-env-changed=SWOOLE_ORACLE_DIR");
            let dir = PathBuf::from(
                env::var("SWOOLE_ORACLE_DIR").unwrap_or_else(|_| "/usr/local/instantclient".into()),
            );
            // AC_PDO_OCI_VERSION: libclntsh.so.19.1 is client 19.1.
            let version = fs::read_dir(&dir)
                .expect("ext: read the Instant Client directory ($SWOOLE_ORACLE_DIR)")
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter_map(|name| {
                    Some(
                        name.strip_prefix("libclntsh.so.")?
                            .strip_suffix(".1")?
                            .to_owned(),
                    )
                })
                .max()
                .expect("ext: no libclntsh.so.*.1 in the Instant Client directory");
            p.define(&[
                "SW_USE_ORACLE",
                "HAVE_OCIENVCREATE",
                "HAVE_OCIENVNLSCREATE",
                "HAVE_OCISTMTFETCH2",
                "HAVE_OCILOBREAD2",
            ]);
            p.defines
                .push(format!(r#"SWOOLE_PDO_OCI_CLIENT_VERSION "{version}.1""#));
            p.c.extend(["oci_driver.c", "oci_statement.c"].map(|f| third.join("pdo_oci").join(f)));
            p.includes.push(dir.join("sdk/include"));
            println!("cargo::rustc-link-search=native={}", dir.display());
            p.libs.push("clntsh");
        }
        if feature("sqlite") {
            p.define(&[
                "SW_USE_SQLITE",
                "HAVE_SW_PDO_SQLITELIB",
                "HAVE_SW_SQLITE3_CLOSE_V2",
                "HAVE_SW_SQLITE3_COLUMN_TABLE_NAME",
            ]);
            let dir = third.join("pdo_sqlite");
            p.c.extend(["sqlite_driver.c", "sqlite_statement.c"].map(|f| dir.join(f)));
            if php >= 84 {
                p.c.push(dir.join("sqlite_sql_parser.c"));
            }
            p.includes.extend(pkg_includes("sqlite3"));
            p.libs.push("sqlite3");
        }
        if feature("firebird") {
            p.define(&["SW_USE_FIREBIRD"]);
            let dir = third
                .join(if php >= 85 { "php85" } else { "php84" })
                .join("pdo_firebird");
            p.c.extend(["firebird_driver.c", "firebird_statement.c"].map(|f| dir.join(f)));
            p.cxx.push(dir.join("pdo_firebird_utils.cpp"));
            p.libs.push("fbclient");
        }
        if feature("ssh2") {
            p.define(&[
                "SW_HAVE_SSH2LIB",
                "PHP_SSH2_AGENT_AUTH",
                "PHP_SSH2_SESSION_TIMEOUT",
            ]);
            let dir = third.join("php/ssh2");
            p.cxx
                .extend(["ssh2.cc", "ssh2_fopen_wrappers.cc", "ssh2_sftp.cc"].map(|f| dir.join(f)));
            p.libs.push("ssh2");
        }
        p
    }

    /// The `-I` directories a command prints.
    fn include_flags(command: &mut Command) -> Option<Vec<PathBuf>> {
        let output = command.output().ok().filter(|o| o.status.success())?;
        Some(
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .filter_map(|flag| flag.strip_prefix("-I"))
                .map(PathBuf::from)
                .collect(),
        )
    }

    /// PHP's headers, and PCRE2's, which ext/pcre's include (as `build_upstream.py` adds them).
    fn php_includes(php_config: &str) -> Vec<PathBuf> {
        let mut includes = include_flags(Command::new(php_config).arg("--includes"))
            .expect("ext: php-config --includes ($PHP_CONFIG) failed");
        includes.extend(
            include_flags(Command::new("pkg-config").args(["--cflags", "libpcre2-8"]))
                .unwrap_or_default(),
        );
        includes
    }

    fn sources(directory: &Path, extension: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(directory)
            .expect("ext: read a source directory")
            .map(|entry| entry.expect("ext: directory entry").path())
            .filter(|path| path.extension().is_some_and(|e| e == extension))
            .collect();
        found.sort();
        found
    }

    /// boost.context's switch, as config.m4 picks it.
    fn context_asm(os: &str, arch: &str) -> &'static str {
        match (os, arch) {
            ("macos", _) => "combined_sysv_macho_gas.S",
            (_, "x86_64") => "x86_64_sysv_elf_gas.S",
            (_, "aarch64") => "arm64_aapcs_elf_gas.S",
            (_, "riscv64") => "riscv64_sysv_elf_gas.S",
            (_, "loongarch64") => "loongarch64_sysv_elf_gas.S",
            _ => panic!("ext: no boost.context switch for {os}/{arch}"),
        }
    }

    pub(super) fn build() {
        let Some(tree) = env::var_os("SWOOLE_SRC").map(PathBuf::from) else {
            println!(
                "cargo::warning=SWOOLE_SRC is unset: swoole.so links no native objects and will not load (tools/build_ext.py sets it)"
            );
            return;
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let native = root.join("native");
        let out = PathBuf::from(env::var("OUT_DIR").expect("ext: OUT_DIR"));
        let os = env::var("CARGO_CFG_TARGET_OS").expect("ext: target OS");
        let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("ext: target arch");
        let platform = if os == "macos" { MACOS } else { LINUX };
        let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".into());
        let third = tree.join("thirdparty");
        let owned = native.join("swoole-src");
        let vendored = owned.join("thirdparty");
        let profile = profile(&vendored, php_version_id(&php_config));
        // config.m4: thread context replaces boost.context's assembly switch.
        let asm_context = !feature("thread_context");
        let config: String = COMMON
            .iter()
            .chain(platform)
            .chain(asm_context.then_some(&"SW_USE_ASM_CONTEXT"))
            .map(|name| format!("#define {name} 1\n"))
            .chain(
                profile
                    .defines
                    .iter()
                    .map(|define| format!("#define {define}\n")),
            )
            .collect();
        fs::write(out.join("config.h"), config).expect("ext: write config.h");
        println!("cargo::rerun-if-changed={}", native.display());

        let mut includes = vec![out, native.join("include")];
        let mut modules: Vec<PathBuf> = fs::read_dir(&native)
            .expect("ext: read native/")
            .map(|entry| entry.expect("ext: native entry").path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .is_some_and(|n| n != "include" && n != "swoole-src")
            })
            .collect();
        modules.sort();
        includes.extend(modules.iter().cloned());
        includes.extend([owned.clone(), owned.join("thirdparty")]);
        // The pin's headers, which only the profile's PHP-derived thirdparty sources read (#2660).
        if !profile.c.is_empty() || !profile.cxx.is_empty() {
            includes.extend([tree.clone(), tree.join("ext-src"), third.clone()]);
        }
        includes.extend(env::var_os("DEP_OPENSSL_INCLUDE").map(PathBuf::from));
        includes.extend(php_includes(&php_config));
        includes.extend(profile.includes);
        // native/phphelpers/host.c reads the zstd version (zstd-sys links libzstd).
        includes.extend(pkg_includes("libzstd"));

        let base = |cpp: bool| {
            let mut build = cc::Build::new();
            build
                .cpp(cpp)
                .includes(&includes)
                .define("HAVE_CONFIG_H", None)
                .define("ENABLE_PHP_SWOOLE", None)
                .define("ZEND_COMPILE_DL_EXT", "1")
                .warnings(false)
                // Objects with no caller in Rust still register handlers.
                .link_lib_modifier("+whole-archive");
            build
        };

        let mut cxx = base(true);
        cxx.flag("-std=c++14").flag("-fexceptions");
        for module in &modules {
            cxx.files(sources(module, "cc"));
        }
        cxx.file(vendored.join("php/standard/proc_open.cc"))
            .files(profile.cxx)
            .compile("swoole_native_cxx");

        let asm = context_asm(&os, &arch);
        let mut c = base(false);
        for module in &modules {
            c.files(sources(module, "c"));
        }
        c.file(vendored.join("php/streams/plain_wrapper.c"))
            .files(["api.c", "http.c", "llhttp.c"].map(|f| third.join("llhttp").join(f)))
            // Rust's coroutine links boost.context's switch even under thread context.
            .file(vendored.join("boost/asm").join(format!("make_{asm}")))
            .file(vendored.join("boost/asm").join(format!("jump_{asm}")))
            .files(profile.c)
            .compile("swoole_native_c");

        for library in profile.libs.into_iter().chain(["z", "pthread", "m"]) {
            println!("cargo::rustc-link-lib={library}");
        }
        if os == "linux" {
            // Last on the line: dependency rlibs, linked after this crate's libraries, call
            // operator new too, and --as-needed drops an earlier -lstdc++. Ubuntu's PHP loads
            // libstdc++ itself (through ICU), which hides this; Alpine's doesn't.
            println!("cargo::rustc-cdylib-link-arg=-lstdc++");
            println!("cargo::rustc-link-lib=dl");
        }
    }
}
