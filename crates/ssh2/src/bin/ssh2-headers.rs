//! Emit the C dispatch bridge from the typed Rust hook and alias inventory.
#[expect(
    clippy::print_stdout,
    reason = "stdout is the header consumed by tools/headers.sh"
)]
fn main() {
    println!(
        "/* Generated from crates/ssh2 by tools/headers.sh; do not edit. */\n{}{}",
        swoole_rs_ssh2::NATIVE_DISPATCH,
        swoole_rs_ssh2::ALIAS_DISPATCH
    );
}
