//! Generate Zend bindings from `php-config --includes` and compile the C shim.

fn main() {
    println!("cargo::rerun-if-env-changed=PHP_CONFIG");
    #[cfg(feature = "php")]
    php::bind();
}

#[cfg(feature = "php")]
#[expect(
    clippy::expect_used,
    reason = "a build script reports failure by panicking"
)]
mod php {
    use std::{env, path::PathBuf, process::Command};

    fn includes() -> Vec<PathBuf> {
        let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".into());
        let output = Command::new(&php_config)
            .arg("--includes")
            .output()
            .expect("zend-sys: cannot run php-config ($PHP_CONFIG)");
        assert!(
            output.status.success(),
            "zend-sys: {php_config} --includes failed"
        );
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|flag| flag.strip_prefix("-I"))
            .map(PathBuf::from)
            .collect()
    }

    pub(super) fn bind() {
        // Every family adds its own src/shim_<family>.{h,c}; nothing here
        // lists them, so parallel changes never touch this file.
        let mut sources: Vec<PathBuf> = std::fs::read_dir("src")
            .expect("zend-sys: read src")
            .map(|entry| entry.expect("zend-sys: src entry").path())
            .filter(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("c" | "h")))
            .collect();
        sources.sort();
        // The directory too: a new shim must rerun this.
        println!("cargo::rerun-if-changed=src");
        println!("cargo::rerun-if-changed=../../native/corebase/swoole_rs_zend_guard.h");
        for file in &sources {
            println!("cargo::rerun-if-changed={}", file.display());
        }
        let includes = includes();
        let out = PathBuf::from(env::var("OUT_DIR").expect("zend-sys: OUT_DIR"));
        bindgen::Builder::default()
            .headers(sources.iter().filter(|p| p.extension().is_some_and(|e| e == "h")).map(|p| p.display().to_string()))
            .generate_comments(false)
            .clang_args(includes.iter().map(|dir| format!("-I{}", dir.display())))
            .allowlist_function("(OnUpdateBool|OnUpdateLong|_?zend_|object_properties_init|object_init_ex|php_error_docref|php_url_encode|php_format_date|php_time|swr_zend_|swr_zval_|zval_|get_active_|add_assoc_|add_next_index_|add_index_).*")
            .allowlist_type("(_?zend_|_?zval|_?php_stream_xport_).*")
            .allowlist_var(
                "(PHP_VERSION_ID|std_object_handlers|module_registry|zend_ce_.*|zend_empty_(array|string)|ZEND_ACC_.*|IS_.*|_IS_BOOL|E_.*|ZPP_ERROR_.*|CONST_.*|MAY_BE_.*|_ZEND_(TYPE|IS|SEND)_.*|PHP_STREAM_(FREE|FLAG)_.*|ZEND_(ASSIGN_DIM|ASSIGN_DIM_OP|INIT_METHOD_CALL|UNSET_DIM|FE_RESET_RW|CONCAT))",
            )
            // PHP 8.5's tail-call VM types opcode handlers preserve_none, which
            // bindgen cannot express; every VM kind but SWITCH stores a pointer,
            // and the generated layout assertions catch a SWITCH build.
            .blocklist_type("zend_vm_opcode_handler_t")
            .raw_line("pub type zend_vm_opcode_handler_t = *const ::std::ffi::c_void;")
            .generate()
            .expect("zend-sys: bindgen")
            .write_to_file(out.join("bindings.rs"))
            .expect("zend-sys: write bindings");
        cc::Build::new()
            .files(
                sources
                    .iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "c")),
            )
            .includes(&includes)
            // Run native scope cleanup when a C++ caller unwinds through a C
            // shim. Zend longjmp uses the explicit cleanup in the guard macros.
            .flag("-fexceptions")
            .warnings(true)
            .compile("swoole_rs_zend_shim");
    }
}
