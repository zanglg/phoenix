use std::env;
use std::path::PathBuf;

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/entry.S");

    if env::var("TARGET").expect("Cargo must provide TARGET") == AARCH64_TARGET {
        let script = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
        )
        .join("linker.ld");
        println!(
            "cargo:rustc-link-arg-bin=phoenix-init=-T{}",
            script.display()
        );
        println!("cargo:rustc-link-arg-bin=phoenix-init=--build-id=none");
        println!("cargo:rustc-link-arg-bin=phoenix-init=-z");
        println!("cargo:rustc-link-arg-bin=phoenix-init=max-page-size=4096");
    }
}
