//! Emit native/corebase/switches.cc: the build's switches, as
//! `swr_cpp_switches` bits, and the bug report's compile-time lines (`tools/headers.sh`).
#[path = "../switches.rs"]
pub mod switches;
#[expect(clippy::print_stdout, reason = "stdout is the generated native source")]
fn main() {
    println!(
        "// Generated from crates/corebase/src/switches.rs by tools/headers.sh; do not edit.\n\
         // The build's switches, which Rust reads (swoole_rs_corebase::built_with): many are\n\
         // the TLS library's header macros, which only a compile of its headers sees.\n\
         #include \"swoole_ssl.h\"\n\n\
         extern \"C\" const uint64_t swr_cpp_switches = 0"
    );
    for (switch, condition, _) in switches::SWITCHES {
        println!(
            "#if {condition}\n    | UINT64_C(1) << {} /* {switch:?} */\n#endif",
            *switch as u8
        );
    }
    println!(
        r#"    ;

// The bug report's compile-time lines (swoole_rs_corebase::bug_report_init).
extern "C" const char *const swr_cpp_bug_report[3] = {{
    SWOOLE_BUG_REPORT, __VERSION__, "OPENSSL_VERSION: " OPENSSL_VERSION_TEXT "\n"}};"#
    );
}
