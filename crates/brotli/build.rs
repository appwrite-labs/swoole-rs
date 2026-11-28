#[expect(
    clippy::expect_used,
    reason = "a missing system library is a build error"
)]
fn main() {
    for library in ["libbrotlienc", "libbrotlidec"] {
        pkg_config::Config::new()
            .probe(library)
            .expect("system libbrotli");
    }
}
