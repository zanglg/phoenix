use std::env;
use std::path::PathBuf;

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/entry.S");
    println!("cargo:rerun-if-changed=src/preempt.S");
    println!("cargo:rerun-if-changed=src/sleep.S");
    for source in ["system", "child", "fault", "cooperative"] {
        println!("cargo:rerun-if-changed=src/{source}.S");
    }

    if env::var("TARGET").expect("Cargo must provide TARGET") == AARCH64_TARGET {
        let script = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
        )
        .join("linker.ld");
        for binary in [
            "phoenix-system",
            "phoenix-child",
            "phoenix-fault",
            "phoenix-cooperative",
        ] {
            println!("cargo:rustc-link-arg-bin={binary}=-T{}", script.display());
            println!("cargo:rustc-link-arg-bin={binary}=--build-id=none");
            println!("cargo:rustc-link-arg-bin={binary}=-z");
            println!("cargo:rustc-link-arg-bin={binary}=max-page-size=4096");
        }
        println!(
            "cargo:rustc-link-arg-bin=phoenix-init=-T{}",
            script.display()
        );
        println!("cargo:rustc-link-arg-bin=phoenix-init=--build-id=none");
        println!("cargo:rustc-link-arg-bin=phoenix-init=-z");
        println!("cargo:rustc-link-arg-bin=phoenix-init=max-page-size=4096");
    }
}
