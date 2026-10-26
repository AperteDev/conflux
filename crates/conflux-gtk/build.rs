//! Compiles the Blueprint views in `ui/` to GtkBuilder XML in OUT_DIR, where
//! `src/ui.rs` includes them, and the app's icons to a resource bundle there,
//! which `src/main.rs` embeds.

use std::process::Command;

fn main() {
    println!("cargo::rerun-if-env-changed=CONFLUX_HELPER_PATH");
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

    println!("cargo::rerun-if-changed=resources.gresource.xml");
    println!("cargo::rerun-if-changed=../../data/icons");
    let ok = Command::new("glib-compile-resources")
        .args(["--sourcedir", ".", "--target"])
        .arg(format!("{out}/resources.gresource"))
        .arg("resources.gresource.xml")
        .status()
        .expect("glib-compile-resources not found; install GLib's development tools")
        .success();
    assert!(ok, "glib-compile-resources failed");
}
