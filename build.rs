use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    } else {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let rc = out.join("manifest.rc");
        let object = out.join("manifest.o");
        std::fs::write(
            &rc,
            format!(
                "1 24 \"{}\"\n",
                manifest.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();
        let status = Command::new(env::var("WINDRES").unwrap_or_else(|_| "windres".into()))
            .arg("-i")
            .arg(rc)
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&object)
            .status()
            .expect("Install windres or set WINDRES to its executable path");
        assert!(status.success(), "Failed to compile application manifest");
        println!("cargo:rustc-link-arg={}", object.display());
    }
}
