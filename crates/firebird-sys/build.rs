//! With `ibase`, bind the system ibase.h's descriptor layouts (`XSQLDA`,
//! `XSQLVAR`) and SQL type codes. The extension compiles the driver against the
//! same header (firebird-dev); `CPATH`'s directories are searched as the C
//! compiler searches them.

fn main() {
    #[cfg(feature = "ibase")]
    ibase::bind();
}

#[cfg(feature = "ibase")]
#[expect(
    clippy::expect_used,
    reason = "a build script reports failure by panicking"
)]
mod ibase {
    use std::{env, path::PathBuf};

    pub(super) fn bind() {
        println!("cargo::rerun-if-env-changed=CPATH");
        let out = PathBuf::from(env::var("OUT_DIR").expect("firebird-sys: OUT_DIR"));
        let search = env::var("CPATH").unwrap_or_default();
        bindgen::Builder::default()
            .header_contents("ibase_bindings.h", "#include <ibase.h>\n")
            .generate_comments(false)
            .clang_args(
                search
                    .split(':')
                    .filter(|dir| !dir.is_empty())
                    .map(|dir| format!("-I{dir}")),
            )
            .allowlist_type("XSQLDA|XSQLVAR|ISC_TIMESTAMP")
            .allowlist_var("DSQL_close|DSQL_drop|isc_info_end|isc_info_req_select_count|isc_info_sql_records")
            .allowlist_var("isc_info_sql_stmt_(insert|update|delete|exec_procedure)")
            .allowlist_var("SQL_(TEXT|VARYING|SHORT|LONG|INT64|FLOAT|DOUBLE|BOOLEAN|TYPE_DATE|TYPE_TIME|TIMESTAMP)")
            .generate()
            .expect("firebird-sys: bindgen")
            .write_to_file(out.join("ibase.rs"))
            .expect("firebird-sys: write bindings");
    }
}
