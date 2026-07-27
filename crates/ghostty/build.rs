use std::path::PathBuf;

/// Where `scripts/build-oracle.sh` installs libghostty-vt, unless overridden.
const DEFAULT_VENDOR: &str = "vendor/libghostty-vt";
const OVERRIDE_ENV: &str = "VTR_LIBGHOSTTY_VT_DIR";

fn main() {
    let vendor = locate_vendor();
    let include = vendor.join("include");
    let lib = vendor.join("lib");

    if !lib.join("libghostty-vt.a").exists() {
        panic!(
            "libghostty-vt is not built.\n  expected: {}\n  fix: ./scripts/build-oracle.sh\n\
             (or set {OVERRIDE_ENV} to a prefix containing include/ and lib/)",
            lib.join("libghostty-vt.a").display()
        );
    }

    println!("cargo:rerun-if-env-changed={OVERRIDE_ENV}");
    println!("cargo:rerun-if-changed=src/wrapper.h");
    println!("cargo:rerun-if-changed={}", lib.join("libghostty-vt.a").display());

    // The static archive is deliberate: the dylib ships with no LC_RPATH, so anything
    // linking it aborts at startup unless DYLD_LIBRARY_PATH happens to be set.
    //
    // Searching a directory holding only the archive is not belt-and-braces. With both
    // libghostty-vt.a and libghostty-vt.dylib visible on one search path, `-l static=`
    // held for every target here except the lib test harness, which silently picked the
    // dylib and died in dyld. Isolating the archive removes the choice.
    let link_dir = out_dir().join("link");
    std::fs::create_dir_all(&link_dir).expect("cannot create the link directory");
    let archive = link_dir.join("libghostty-vt.a");
    let _ = std::fs::remove_file(&archive);
    std::os::unix::fs::symlink(lib.join("libghostty-vt.a"), &archive)
        .expect("cannot link the vendored archive into OUT_DIR");

    println!("cargo:rustc-link-search=native={}", link_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    let bindings = bindgen::Builder::default()
        .header("src/wrapper.h")
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg("-DGHOSTTY_STATIC")
        .allowlist_function("ghostty_terminal_(new|free|vt_write|get|grid_ref)")
        .allowlist_function("ghostty_grid_ref_(cell|row|graphemes|style)")
        .allowlist_function("ghostty_(cell|row)_get")
        .allowlist_function("ghostty_style_default")
        .allowlist_function("ghostty_type_json")
        .allowlist_type("Ghostty(Result|TerminalData|TerminalScreen)")
        .allowlist_type("Ghostty(CellData|CellWide|CellContentTag|RowData)")
        .allowlist_type("Ghostty(PointTag|StyleColorTag)")
        .allowlist_type("Ghostty(SgrUnderline)")
        .layout_tests(true)
        .generate()
        .expect("bindgen failed against the vendored libghostty-vt headers");

    bindings
        .write_to_file(out_dir().join("bindings.rs"))
        .expect("failed to write generated bindings");
}

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set by cargo"))
}

fn locate_vendor() -> PathBuf {
    if let Ok(dir) = std::env::var(OVERRIDE_ENV) {
        return PathBuf::from(dir);
    }
    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo"),
    );
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crate lives at <workspace>/crates/ghostty")
        .join(DEFAULT_VENDOR)
}
