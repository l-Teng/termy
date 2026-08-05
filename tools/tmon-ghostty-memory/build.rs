use std::{env, path::PathBuf};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        panic!("tools/tmon-ghostty-memory is macOS-only");
    }

    let prefix = PathBuf::from(
        env::var("GHOSTTY_VT_PREFIX")
            .expect("GHOSTTY_VT_PREFIX must point to a libghostty-vt installation"),
    );
    let include_dir = prefix.join("include");
    let lib_dir = prefix.join("lib");
    let source = PathBuf::from("src/ghostty_shim.c");

    if !include_dir.join("ghostty/vt.h").exists() {
        panic!(
            "GHOSTTY_VT_PREFIX is missing ghostty headers at {}",
            include_dir.display()
        );
    }
    if !lib_dir.join("libghostty-vt.a").exists() {
        panic!(
            "GHOSTTY_VT_PREFIX is missing lib/libghostty-vt.a at {}",
            lib_dir.display()
        );
    }
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_PREFIX");
    println!("cargo:rerun-if-changed={}", source.display());
    println!(
        "cargo:rerun-if-changed={}",
        include_dir.join("ghostty/vt.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        lib_dir.join("libghostty-vt.a").display()
    );

    cc::Build::new()
        .file(&source)
        .include(&include_dir)
        .define("GHOSTTY_STATIC", None)
        .flag_if_supported("-std=c11")
        .compile("ghostty_shim");

    println!(
        "cargo:rustc-link-arg={}",
        lib_dir.join("libghostty-vt.a").display()
    );
}
