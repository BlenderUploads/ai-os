//! Compiles the vendored DOOM engine when the `doom` feature is on.
//!
//! The engine is GPL-2 and the rest of HALCYON is MIT, so this is deliberately
//! opt-in: a default `make iso` never touches `doom/` and produces an
//! MIT-licensed kernel. See doom/README.md.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DOOM");

    if std::env::var("CARGO_FEATURE_DOOM").is_err() {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let doom = manifest.parent().unwrap().join("doom");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    if !doom.join("src").is_dir() {
        panic!(
            "the `doom` feature is on but {} is missing; run tools/fetch-doom.sh",
            doom.join("src").display()
        );
    }

    println!("cargo:rerun-if-changed={}", doom.join("src").display());
    println!("cargo:rerun-if-changed={}", doom.join("libc.c").display());
    println!("cargo:rerun-if-changed={}", doom.join("sound.c").display());
    println!("cargo:rerun-if-changed={}", doom.join("include").display());

    let compiler = std::env::var("CC").unwrap_or_else(|_| "gcc".to_string());

    // gcc still ships the freestanding headers (stddef, stdint, stdarg); the
    // hosted ones come from doom/include instead.
    let freestanding = Command::new(&compiler)
        .arg("-print-file-name=include")
        .output()
        .expect("could not run the C compiler");
    let freestanding = String::from_utf8_lossy(&freestanding.stdout)
        .trim()
        .to_string();

    let flags: Vec<String> = vec![
        "-c".into(),
        "-O2".into(),
        "-ffreestanding".into(),
        "-fno-stack-protector".into(),
        "-fno-stack-clash-protection".into(),
        "-fno-pic".into(),
        // The kernel lives in the top 2 GiB, and DOOM links into it.
        "-mcmodel=kernel".into(),
        "-mno-red-zone".into(),
        "-mno-mmx".into(),
        // DOOM's start-up table generation uses doubles. SSE keeps that
        // straightforward; the scheduler saves the vector registers across
        // context switches so this cannot disturb anything else.
        "-msse".into(),
        "-msse2".into(),
        "-nostdinc".into(),
        "-isystem".into(),
        freestanding,
        "-I".into(),
        doom.join("include").display().to_string(),
        "-I".into(),
        doom.join("src").display().to_string(),
        "-w".into(),
        "-DNORMALUNIX".into(),
        // Compiles in the sound path; doom/sound.c supplies the module it
        // expects the platform to define.
        "-DFEATURE_SOUND".into(),
        "-DDOOMGENERIC_RESX=640".into(),
        "-DDOOMGENERIC_RESY=400".into(),
    ];

    let mut objects = Vec::new();
    let mut sources: Vec<PathBuf> = std::fs::read_dir(doom.join("src"))
        .expect("doom/src unreadable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("c"))
        .collect();
    // Everything outside doom/src is HALCYON's own: the C library the engine
    // links against and the sound module it asks a platform to supply.
    sources.push(doom.join("libc.c"));
    sources.push(doom.join("sound.c"));
    sources.sort();

    for source in &sources {
        let stem = source.file_stem().unwrap().to_string_lossy().to_string();
        let object = out.join(format!("{}.o", stem));
        let status = Command::new(&compiler)
            .args(&flags)
            .arg(source)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("could not run the C compiler");
        if !status.success() {
            panic!("failed to compile {}", source.display());
        }
        objects.push(object);
    }

    let archive = out.join("libdoom.a");
    let _ = std::fs::remove_file(&archive);
    let status = Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .args(&objects)
        .status()
        .expect("could not run ar");
    if !status.success() {
        panic!("failed to archive the DOOM objects");
    }

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=doom");
    let _ = Path::new("/");
}
