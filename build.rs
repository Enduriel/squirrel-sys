extern crate bindgen;
extern crate cmake;

use std::env;
use std::path::PathBuf;

/// Returns the default C++ standard library for the current target: `libc++`
/// for OS X and `libstdc++` for anything else.
///
/// Credits: cc-rs crate
fn get_cpp_link_stdlib() -> Option<String> {
    if let Ok(stdlib) = env::var("CXXSTDLIB") {
        if stdlib.is_empty() {
            None
        } else {
            Some(stdlib)
        }
    } else {
        let target = env::var("TARGET").unwrap();
        if target.contains("msvc") {
            None
        } else if target.contains("apple")
            || target.contains("freebsd")
            || target.contains("openbsd")
        {
            Some("c++".to_string())
        } else {
            Some("stdc++".to_string())
        }
    }
}

fn main() {
    let double = env::var_os("CARGO_FEATURE_DOUBLE").is_some();

    let mut config = cmake::Config::new("squirrel");
    if double {
        config.cxxflag("-DSQUSEDOUBLE");
    }
    // SQ_ASAN=1: instrument the C++ with AddressSanitizer (for debugging memory
    // errors in the VM). The binary must also link asan
    // (RUSTFLAGS="-Clink-arg=-fsanitize=address").
    if env::var_os("SQ_ASAN").is_some() {
        config
            .cflag("-fsanitize=address")
            .cflag("-fno-omit-frame-pointer")
            .cflag("-g")
            .cxxflag("-fsanitize=address")
            .cxxflag("-fno-omit-frame-pointer")
            .cxxflag("-g")
            .define("CMAKE_BUILD_TYPE", "Debug");
    }
    let dst = config.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    // sqstdlib (string/math/regexp stdlib) depends on the core VM, so it is listed
    // first — the linker resolves symbols left-to-right.
    println!("cargo:rustc-link-lib=static=sqstdlib_static");
    println!("cargo:rustc-link-lib=static=squirrel_static");
    if let Some(stdlib) = get_cpp_link_stdlib() {
        println!("cargo:rustc-flags=-l dylib={}", stdlib);
    }

    let mut builder = bindgen::Builder::default().header("wrapper.h");
    if double {
        builder = builder.clang_arg("-DSQUSEDOUBLE");
    }
    let bindings = builder
        .trust_clang_mangling(false)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
