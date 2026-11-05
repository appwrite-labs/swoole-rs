//! Emit only native compile-capability projection; Rust owns information rows.
#[path = "../info_inventory.rs"]
mod inventory;
#[expect(clippy::print_stdout, reason = "stdout is the generated native header")]
fn main() {
    println!(
        "/* Generated from phpswoole's capability inventory; do not edit. */\n#ifndef SWOOLE_RS_PHPSWOOLE_INFO_H\n#define SWOOLE_RS_PHPSWOOLE_INFO_H\n#include <stdint.h>\nstatic uint64_t swr_cpp_phpswoole_info_flags() {{\n    uint64_t flags = 0;"
    );
    for (capability, condition, label) in inventory::CAPABILITIES {
        println!(
            "#if {condition}\n    flags |= UINT64_C(1) << {}; /* {:?} */\n#endif",
            *capability as u8, label
        );
    }
    println!("    return flags;\n}}\n#endif");
}
