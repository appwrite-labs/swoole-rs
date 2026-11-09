//! Emit native/phphelpers/switches.cc: the build's configure switches, as
//! `swr_cpp_switches` bits (`tools/headers.sh`).
#[path = "../switches.rs"]
pub mod switches;
#[expect(clippy::print_stdout, reason = "stdout is the generated native source")]
fn main() {
    println!(
        "// Generated from crates/corebase/src/switches.rs by tools/headers.sh; do not edit.\n\
         // The build's configure switches, which Rust reads (swoole_rs_corebase::built_with):\n\
         // Cargo builds the Rust archive before configure runs.\n\
         #include \"swoole_rs_phphost.h\"\n\
         #include \"swoole_ssl.h\"\n\n\
         extern \"C\" const uint64_t swr_cpp_switches = 0"
    );
    for (switch, condition, _) in switches::SWITCHES {
        println!(
            "#if {condition}\n    | UINT64_C(1) << {} /* {switch:?} */\n#endif",
            *switch as u8
        );
    }
    println!("    ;");
}
