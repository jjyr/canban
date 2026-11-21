// Adapted from herdr-app e77f112ac2ea136b84fe510efa42d41163bfd776 (Apache-2.0).
//! Builds libghostty-vt from the checksum-pinned .local/deps/ghostty with Zig and generates bindings from its headers.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::process::Command;

// Zig builds for the machine it runs on unless told otherwise: its CPU's
// newest instructions, which crash older CPUs, and never another target's.
fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("ghostty-sys: no Zig target for {other}"),
    }
}

fn cache_root(manifest: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CANBAN_CACHE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            return home.join("Library/Caches/canban");
        }
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|s| !s.is_empty()) {
            return PathBuf::from(xdg).join("canban");
        }
        return home.join(".cache/canban");
    }
    manifest.join("../../.local/cache")
}

/// Deterministic 128-bit FNV-1a digest with domain framing for shared build artifact keys.
struct CacheHasher(u64, u64);

impl CacheHasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325, 0xaf63_bd4c_8601_b7df)
    }

    fn update(&mut self, tag: &[u8], data: &[u8]) {
        self.feed(tag);
        self.feed(&(data.len() as u64).to_le_bytes());
        self.feed(data);
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
            self.1 ^= u64::from(b).wrapping_add(0x9e);
            self.1 = self.1.wrapping_mul(0x0100_0000_01b3).rotate_left(7);
        }
    }

    fn finish_hex(self) -> String {
        format!("{:016x}{:016x}", self.0, self.1)
    }
}

fn cache_key(
    manifest: &Path,
    src: &Path,
    target: &str,
    target_os: &str,
    target_env: &str,
    zig_version: &[u8],
    macos_sdk: &[u8],
) -> String {
    let mut h = CacheHasher::new();
    h.update(b"format", b"ghostty-vt-v1");
    let revision = std::fs::read(src.join(".canban-revision"))
        .or_else(|_| std::fs::read(src.join("build.zig.zon")))
        .unwrap_or_default();
    h.update(b"revision", &revision);
    h.update(
        b"build.zig",
        &std::fs::read(src.join("build.zig")).unwrap_or_default(),
    );
    h.update(
        b"vt.h",
        &std::fs::read(src.join("include/ghostty/vt.h")).unwrap_or_default(),
    );
    h.update(b"target", target.as_bytes());
    h.update(b"target_os", target_os.as_bytes());
    h.update(b"target_env", target_env.as_bytes());
    h.update(b"zig_version", zig_version);
    h.update(b"macos_sdk", macos_sdk);
    h.update(
        b"ffi.h",
        &std::fs::read(manifest.join("src/ffi.h")).unwrap_or_default(),
    );
    h.update(
        b"build.rs",
        &std::fs::read(manifest.join("build.rs")).unwrap_or_default(),
    );
    h.finish_hex()
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap();
    if target_os == "macos" {
        cc::Build::new()
            .file("src/apple_render.m")
            .file("src/macos_view.m")
            .flag("-fobjc-arc")
            .flag("-Wno-incompatible-pointer-types")
            .compile("canban-terminal-native");
        for fw in ["AppKit", "CoreText", "CoreGraphics", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
        println!("cargo:rerun-if-changed=src/apple_render.h");
        println!("cargo:rerun-if-changed=src/apple_render.m");
        println!("cargo:rerun-if-changed=src/macos_view.m");
    }
    println!("cargo:rerun-if-changed=src/ffi.h");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest
        .join("../../.local/deps/ghostty")
        .canonicalize()
        .expect("Ghostty missing: run mise run terminal-deps");
    // Windows' canonical form is `\\?\C:\...`, which clang can't resolve includes against.
    let src = PathBuf::from(src.to_string_lossy().trim_start_matches(r"\\?\").to_owned());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for path in ["src", "build.zig", "build.zig.zon", "include"] {
        println!("cargo:rerun-if-changed={}", src.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=CANBAN_CACHE_DIR");

    let zig = std::env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let zig_version = Command::new(&zig)
        .arg("version")
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let macos_sdk = if target_os == "macos" {
        Command::new("xcrun")
            .args(["--show-sdk-path"])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let shared_cache = cache_root(&manifest);
    let key = cache_key(
        &manifest,
        &src,
        &target,
        &target_os,
        &target_env,
        &zig_version,
        &macos_sdk,
    );
    let archive_name = if target_env == "msvc" {
        "ghostty-vt.lib"
    } else {
        "libghostty-vt.a"
    };
    let cache_bucket = shared_cache.join("ghostty-vt");
    let cache_entry = cache_bucket.join(&key);
    let lib = out.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let dst_archive = lib.join(archive_name);
    let dst_bindings = out.join("bindings.rs");

    if non_empty(&cache_entry.join(archive_name))
        && non_empty(&cache_entry.join("bindings.rs"))
        && std::fs::copy(cache_entry.join(archive_name), &dst_archive).is_ok()
        && std::fs::copy(cache_entry.join("bindings.rs"), &dst_bindings).is_ok()
    {
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=ghostty-vt");
        return;
    }

    let status = Command::new(&zig)
        .current_dir(&src)
        // Only the static library is linked. Ghostty builds the xcframework whenever it finds
        // xcodebuild, and with only the Command Line Tools /usr/bin/xcodebuild exists but fails.
        .args([
            "build",
            "-Demit-lib-vt",
            "-Demit-xcframework=false",
            "-Doptimize=ReleaseFast",
        ])
        .arg(format!("-Dtarget={}", zig_target(&target)))
        .arg("--prefix")
        .arg(out.join("zig-out"))
        // Share immutable dependency/compiler objects across worktrees and Cargo feature sets.
        // Keep each invocation's local build cache and install prefix separate.
        .env("ZIG_GLOBAL_CACHE_DIR", shared_cache.join("zig"))
        .arg("--cache-dir")
        .arg(out.join("zig-cache"))
        .status()
        .unwrap_or_else(|e| panic!("cannot run {zig:?} (Zig 0.17 required): {e}"));
    assert!(
        status.success(),
        "zig build of libghostty-vt failed: {status}"
    );

    // Only the static archive in the search path, so the linker can't pick the shared one.
    if target_env == "msvc" {
        std::fs::copy(out.join("zig-out/lib/ghostty-vt-static.lib"), &dst_archive).unwrap();
    } else {
        std::fs::copy(out.join("zig-out/lib/libghostty-vt.a"), &dst_archive).unwrap();
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    // libclang without a full clang install lacks its builtin headers; Zig ships the same ones.
    let zig_env = Command::new(&zig).arg("env").output().expect("zig env");
    let zig_env = String::from_utf8_lossy(&zig_env.stdout);
    let zig_lib = zig_env
        .lines()
        .find_map(|l| l.trim().strip_prefix(".lib_dir = \""))
        .and_then(|l| l.strip_suffix("\","))
        .expect("zig env has no lib_dir");

    let include = src.join("include");
    let mut builder = bindgen::Builder::default()
        .header(manifest.join("src/ffi.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()));
    if target_os == "macos" {
        // Apple's libclang has its builtin headers but needs the SDK for the C
        // library (Zig's builtin headers clash with it there).
        let sdk = String::from_utf8_lossy(&macos_sdk).trim().to_owned();
        if !sdk.is_empty() {
            builder = builder.clang_arg(format!("-isysroot{sdk}"));
        }
    } else if Command::new("clang").arg("--version").output().is_err() {
        // With a clang installed, bindgen asks it for its own builtin headers,
        // which match its libclang; Zig's newer ones break older ones (Ubuntu
        // 22.04's clang 14 can't parse them).
        builder = builder.clang_arg(format!("-isystem{zig_lib}/include"));
    }
    builder
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .allowlist_var("CANBAN_STYLE_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on ghostty/vt.h")
        .write_to_file(&dst_bindings)
        .unwrap();

    if std::fs::create_dir_all(&cache_bucket).is_ok() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_dir = cache_bucket.join(format!(".tmp-{}-{nanos}", std::process::id()));
        if std::fs::create_dir_all(&tmp_dir).is_ok()
            && std::fs::copy(&dst_archive, tmp_dir.join(archive_name)).is_ok()
            && std::fs::copy(&dst_bindings, tmp_dir.join("bindings.rs")).is_ok()
        {
            if std::fs::rename(&tmp_dir, &cache_entry).is_err() {
                let _ = std::fs::remove_dir_all(&tmp_dir);
            }
        } else {
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }
    }
}
