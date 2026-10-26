//! Compiles the Blueprint views in `ui/` to GtkBuilder XML in OUT_DIR, where
//! `src/ui.rs` includes them.

use std::process::Command;

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    println!("cargo::rerun-if-changed=ui");
    let blps: Vec<_> = std::fs::read_dir("ui")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "blp"))
        .collect();
    let ok = Command::new("blueprint-compiler")
        .args(["batch-compile", &out, "ui"])
        .args(&blps)
        .status()
        .expect("blueprint-compiler not found; install it to build conflux-gtk")
        .success();
    assert!(ok, "blueprint-compiler failed");
}
