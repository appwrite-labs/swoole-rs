//! Bind the system unixODBC that pkg-config finds. The extension links it itself
//! (`config.m4`'s `--with-swoole-odbc`); the probe links it for this
//! workspace's own tests.

fn main() {
    #[cfg(feature = "unixodbc")]
    odbc::bind();
}

#[cfg(feature = "unixodbc")]
#[expect(
    clippy::expect_used,
    reason = "a build script reports failure by panicking"
)]
mod odbc {
    use std::{env, path::PathBuf};

    pub(super) fn bind() {
        println!("cargo::rerun-if-changed=src/odbc.h");
        let library = pkg_config::Config::new()
            .probe("odbc")
            .expect("odbc-sys: unixODBC (pkg-config odbc)");
        let out = PathBuf::from(env::var("OUT_DIR").expect("odbc-sys: OUT_DIR"));
        bindgen::Builder::default()
            .header("src/odbc.h")
            .generate_comments(false)
            .clang_args(
                library
                    .include_paths
                    .iter()
                    .map(|dir| format!("-I{}", dir.display())),
            )
            .allowlist_function("SQL.*")
            .allowlist_type("(SQL|RETCODE).*")
            .generate()
            .expect("odbc-sys: bindgen")
            .write_to_file(out.join("bindings.rs"))
            .expect("odbc-sys: write bindings");
    }
}
