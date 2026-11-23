//! With `ibase`, bind the system ibase.h's descriptor layouts (`XSQLDA`,
//! `XSQLVAR`) and SQL type codes. The extension compiles the driver against the
//! same header (firebird-dev); `CPATH`'s directories are searched as the C
//! compiler searches them. The header's `FB_API_VER` is passed on to dependents'
//! build scripts as `DEP_FBCLIENT_API_VER`: the driver's time-zone values exist
//! only from 40, as the C compiled them only then.

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
    use bindgen::callbacks::{IntKind, ParseCallbacks};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::{env, path::PathBuf};

    /// The header's `FB_API_VER`, as bindgen parses it.
    static API_VER: AtomicI64 = AtomicI64::new(0);

    #[derive(Debug)]
    struct ApiVersion;

    impl ParseCallbacks for ApiVersion {
        fn int_macro(&self, name: &str, value: i64) -> Option<IntKind> {
            if name == "FB_API_VER" {
                API_VER.store(value, Ordering::Relaxed);
            }
            None
        }
    }

    pub(super) fn bind() {
        println!("cargo::rerun-if-env-changed=CPATH");
        let out = PathBuf::from(env::var("OUT_DIR").expect("firebird-sys: OUT_DIR"));
        let search = env::var("CPATH").unwrap_or_default();
        bindgen::Builder::default()
            .header_contents("ibase_bindings.h", "#include <ibase.h>\n")
            .generate_comments(false)
            .parse_callbacks(Box::new(ApiVersion))
            .clang_args(
                search
                    .split(':')
                    .filter(|dir| !dir.is_empty())
                    .map(|dir| format!("-I{dir}")),
            )
            .allowlist_type("XSQLDA|XSQLVAR|ISC_TIMESTAMP")
            .allowlist_var("DSQL_close|DSQL_drop|isc_info_end|isc_info_req_select_count|isc_info_sql_records")
            .allowlist_var("isc_info_(truncated|error|blob_total_length)|isc_segment|isc_segstr_eof|SQL_(BLOB|ARRAY)")
            .allowlist_var("isc_info_sql_stmt_(insert|update|delete|exec_procedure)")
            .allowlist_var("SQL_(TEXT|VARYING|SHORT|LONG|INT64|FLOAT|DOUBLE|BOOLEAN|TYPE_DATE|TYPE_TIME|TIMESTAMP)")
            .generate()
            .expect("firebird-sys: bindgen")
            .write_to_file(out.join("ibase.rs"))
            .expect("firebird-sys: write bindings");
        println!(
            "cargo::metadata=api_ver={}",
            API_VER.load(Ordering::Relaxed)
        );
    }
}
