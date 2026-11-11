//! Embeds every file under `/ui` into the program, so the app is one self-contained exe.
//! `/ui/dev` (the mock API and sample data for working on the UI in a browser) is left out.
//!
//! On Windows it also gives the program its icon, version details and manifest
//! (`build/winres.rs`).

#[path = "build/winres.rs"]
mod winres;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Top-level folder of `/ui` that is for development only and never embedded.
const DEV_FOLDER: &str = "dev";

fn collect(dir: &Path, top: bool, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).expect("read ui dir").flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if top && p.file_name().is_some_and(|name| name == DEV_FOLDER) {
                continue;
            }
            collect(&p, false, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let ui = manifest.join("..").join("..").join("ui");
    println!("cargo:rerun-if-changed={}", ui.display());
    let mut files = Vec::new();
    collect(&ui, true, &mut files);
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let rel = f.strip_prefix(&ui).expect("under ui").to_string_lossy().replace('\\', "/");
        writeln!(code, "    ({rel:?}, include_bytes!({:?})),", f.to_string_lossy()).expect("write");
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("assets.rs");
    fs::write(out, code).expect("write assets.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // A compiled .res file goes straight to the MSVC linker and to lld (the gnullvm
        // toolchain). GNU ld would need it converted first, so that build goes without.
        let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
        let llvm = env::var("CARGO_CFG_TARGET_ABI").as_deref() == Ok("llvm");
        if msvc || llvm {
            windows_resources(&manifest);
        } else {
            println!("cargo:warning=this toolchain's linker cannot take a .res file; the program is built without its icon and version details");
        }
    }
}

/// The name shown as the publisher, in the program's details and by the installer
/// (`packaging/installer.iss` carries the same name).
const PUBLISHER: &str = "SInsanali";

fn windows_resources(manifest_dir: &Path) {
    let assets = manifest_dir.join("assets");
    let (ico_path, manifest_path) = (assets.join("vault-key.ico"), assets.join("vault-key.exe.manifest"));
    for input in [&ico_path, &manifest_path, &manifest_dir.join("build").join("winres.rs")] {
        println!("cargo:rerun-if-changed={}", input.display());
    }
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    // Windows wants four plain numbers; a pre-release part (1.0.0-rc.1) is left out there.
    let number = |key: &str| env::var(key).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let numeric = [number("CARGO_PKG_VERSION_MAJOR"), number("CARGO_PKG_VERSION_MINOR"), number("CARGO_PKG_VERSION_PATCH")];
    let numeric_text = format!("{}.{}.{}", numeric[0], numeric[1], numeric[2]);
    let ico = fs::read(&ico_path).expect("read the icon");
    let manifest = fs::read_to_string(&manifest_path).expect("read the manifest").replace("@VERSION@", &numeric_text);
    let info = winres::VersionInfo {
        version: &version,
        numeric,
        product_name: "Vault Key",
        file_description: "Vault Key",
        company_name: PUBLISHER,
        copyright: &format!("Copyright \u{a9} 2026 {PUBLISHER}"),
        original_filename: "vault-key.exe",
    };
    let res = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("vault-key.res");
    winres::write(&res, &ico, &manifest, &info);
    // The linker takes a compiled resource file like any other input.
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
