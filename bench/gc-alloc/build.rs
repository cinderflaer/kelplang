// Link the collector staticlib built from kelpc/gc (cargo build --release there).
// The staticlib defines rust_begin_unwind and rust_eh_personality, which std
// also defines. This bench never panics, so the first definition is harmless.
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kelpc/gc/target/release");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=kelp_gc");
    println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    println!("cargo:rerun-if-changed={}", dir.join("libkelp_gc.a").display());
}
