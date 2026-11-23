//! `fb_api_40` when the ibase.h firebird-sys bound is Firebird 4's or later
//! (`FB_API_VER >= 40`): the driver's time-zone values, whose decoders the
//! extension's C++ (`pdo_firebird_utils.cpp`) defines only then.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(fb_api_40)");
    let version = std::env::var("DEP_FBCLIENT_API_VER").unwrap_or_default();
    if version.parse::<u32>().is_ok_and(|version| version >= 40) {
        println!("cargo::rustc-cfg=fb_api_40");
    }
}
