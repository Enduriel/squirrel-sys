extern crate bindgen;
extern crate cmake;

use std::env;
use std::path::PathBuf;

/// Returns the default C++ standard library for the current target: `libc++`
/// for OS X and for `*-windows-gnullvm` (the llvm-mingw toolchain has no libstdc++),
/// and `libstdc++` for anything else.
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
            || target.ends_with("-windows-gnullvm")
            || target.contains("freebsd")
            || target.contains("openbsd")
        {
            Some("c++".to_string())
        } else {
            Some("stdc++".to_string())
        }
    }
}

/// FNV-1a over every VM source file (in path order) and the build options. It changes
/// whenever the VM or its compiler does, so a cache of compiled scripts can key on it
/// (exposed as `SQUIRREL_SOURCE_HASH`).
fn source_hash(double: bool) -> u64 {
    let mut files = Vec::new();
    for dir in ["squirrel/include", "squirrel/squirrel", "squirrel/sqstdlib"] {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if matches!(path.extension().and_then(|e| e.to_str()), Some("h" | "cpp")) {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for file in files {
        eat(file.to_string_lossy().as_bytes());
        eat(&std::fs::read(&file).unwrap_or_default());
    }
    eat(&[u8::from(double)]);
    hash
}

/// Whether the C/C++ compiler for an msvc `target` is clang-cl (else MSVC's cl): the
/// compiler variables cc-rs (and so cmake-rs) reads, most specific first.
fn clang_cl(target: &str) -> bool {
    let vars = [
        format!("CXX_{target}"),
        format!("CXX_{}", target.replace('-', "_")),
        "TARGET_CXX".to_string(),
        "CXX".to_string(),
    ];
    vars.iter()
        .find_map(|var| env::var(var).ok())
        .is_some_and(|cxx| cxx.contains("clang-cl"))
}

fn main() {
    let double = env::var_os("CARGO_FEATURE_DOUBLE").is_some();
    println!("cargo:rustc-env=SQUIRREL_SOURCE_HASH={:016x}", source_hash(double));

    let mut config = cmake::Config::new("squirrel");
    // Only the static libraries are linked: skip the shared ones and the `sq`
    // interpreter, and pin the install dir (GNUInstallDirs picks lib64 on some
    // distros) to the one searched below.
    config
        .define("DISABLE_DYNAMIC", "ON")
        .define("SQ_DISABLE_INTERPRETER", "ON")
        .define("CMAKE_INSTALL_LIBDIR", "lib");
    if double {
        config.cxxflag("-DSQUSEDOUBLE");
    }
    if env::var_os("CARGO_FEATURE_CENSUS").is_some() {
        config.cxxflag("-DSQ_CENSUS");
    }
    let target = env::var("TARGET").unwrap();
    let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|f| f == "crt-static"));
    // Float results must be the same on every platform (seed-exact scripts): no fused
    // multiply-adds, which clang forms by default where the CPU has them (ARM64). And
    // the VM type-puns (e.g. a float read from an instruction's int slot), which
    // squirrel's CMakeLists guards with -fno-strict-aliasing for GCC only; clang needs
    // it too.
    let flags: &[&str] = if !target.contains("msvc") {
        &["-ffp-contract=off", "-fno-strict-aliasing"]
    } else if clang_cl(&target) {
        // clang-cl (cross builds, cargo-xwin) takes clang's own flags behind `/clang:`.
        &["/clang:-ffp-contract=off", "/clang:-fno-strict-aliasing"]
    } else {
        // MSVC's cl doesn't contract under /fp:precise since VS 2022 17.0 (only with
        // /fp:contract), and has no type-based alias analysis.
        &["/fp:precise"]
    };
    for flag in flags {
        config.cflag(flag).cxxflag(flag);
    }
    if target.contains("msvc") {
        // The C runtime Rust links: the static one under `+crt-static`. CMake's
        // `CMAKE_MSVC_RUNTIME_LIBRARY` needs its policy CMP0091, newer than the
        // CMakeLists' minimum version; without it CMake puts /MD in its release flags
        // and the link mixes two runtimes.
        let runtime = if crt_static { "MultiThreaded" } else { "MultiThreadedDLL" };
        config
            .define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", runtime);
    }
    if target.contains("apple") {
        // Build for the macOS version rustc links for (its default is 11.0), not the
        // SDK's, which the linker warns about for every object.
        let version = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".to_string());
        config.define("CMAKE_OSX_DEPLOYMENT_TARGET", version);
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
        // A static C runtime (`-C target-feature=+crt-static`) on windows-gnullvm links
        // llvm-mingw's libc++ in too, so the program needs no libc++.dll beside it.
        // Not bundled into this crate's rlib: the final link finds it in the toolchain.
        let kind = if crt_static && target.ends_with("-windows-gnullvm") {
            "static:-bundle"
        } else {
            "dylib"
        };
        println!("cargo:rustc-flags=-l {}={}", kind, stdlib);
    }

    let mut builder = bindgen::Builder::default().header("wrapper.h");
    if let Some(arch) = target.strip_suffix("-pc-windows-gnullvm") {
        // bindgen hands rustc's triple to libclang, which can't parse `gnullvm`; the
        // same ABI is clang's `w64-windows-gnu`.
        builder = builder.clang_arg(format!("--target={arch}-w64-windows-gnu"));
    }
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
