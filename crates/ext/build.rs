//! Compile what the extension still needs of C and C++ (native/ and the bundled
//! thirdparty sources config.m4 lists) and link it into `swoole.so`, in place of
//! phpize and configure.

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
    /// OS (the default profile: zlib, no brotli, zstd or sockets).
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
        "SW_HALFCLOSE_DRAIN",
        "SW_HALFCLOSE_PREQUEUE",
        "SW_HAVE_ZLIB",
        "SW_USE_ASM_CONTEXT",
    ];

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
    fn php_includes() -> Vec<PathBuf> {
        let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".into());
        let mut includes = include_flags(Command::new(&php_config).arg("--includes"))
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
        let config: String = COMMON
            .iter()
            .chain(platform)
            .flat_map(|name| ["#define ", name, " 1\n"])
            .collect();
        fs::write(out.join("config.h"), config).expect("ext: write config.h");
        println!("cargo::rerun-if-changed={}", native.display());

        let mut includes = vec![out, native.join("include")];
        let mut modules: Vec<PathBuf> = fs::read_dir(&native)
            .expect("ext: read native/")
            .map(|entry| entry.expect("ext: native entry").path())
            .filter(|path| path.is_dir() && path.file_name().is_some_and(|n| n != "include"))
            .collect();
        modules.sort();
        includes.extend(modules.iter().cloned());
        includes.extend([tree.clone(), tree.join("ext-src"), tree.join("thirdparty")]);
        includes.extend(env::var_os("DEP_OPENSSL_INCLUDE").map(PathBuf::from));
        includes.extend(php_includes());

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

        let third = tree.join("thirdparty");
        let mut cxx = base(true);
        cxx.flag("-std=c++14").flag("-fexceptions");
        for module in &modules {
            cxx.files(sources(module, "cc"));
        }
        cxx.file(third.join("php/standard/var_decoder.cc"))
            .file(third.join("php/standard/proc_open.cc"))
            .compile("swoole_native_cxx");

        let asm = context_asm(&os, &arch);
        let mut c = base(false);
        c.file(third.join("php/streams/plain_wrapper.c"))
            .files(["api.c", "http.c", "llhttp.c"].map(|f| third.join("llhttp").join(f)))
            .file(third.join("multipart_parser.c"))
            .files(
                [
                    "nghttp2_hd.c",
                    "nghttp2_rcbuf.c",
                    "nghttp2_helper.c",
                    "nghttp2_buf.c",
                    "nghttp2_mem.c",
                    "nghttp2_hd_huffman.c",
                    "nghttp2_hd_huffman_data.c",
                ]
                .map(|f| third.join("nghttp2").join(f)),
            )
            .file(third.join("boost/asm").join(format!("make_{asm}")))
            .file(third.join("boost/asm").join(format!("jump_{asm}")))
            .compile("swoole_native_c");

        for library in ["z", "pthread", "m"] {
            println!("cargo::rustc-link-lib={library}");
        }
        if os == "linux" {
            println!("cargo::rustc-link-lib=dl");
        }
    }
}
