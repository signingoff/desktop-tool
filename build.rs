use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
    let icon = manifest.parent().unwrap().join("assets/app.ico");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc = out.join("app.rc");
    let icon_resource = format!(
        "101 ICON \"{}\"\n",
        icon.display().to_string().replace('\\', "/")
    );
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        std::fs::write(&rc, icon_resource).unwrap();
        let resource = out.join("app.res");
        let status = Command::new(env::var("RC").unwrap_or_else(|_| "rc.exe".into()))
            .arg("/nologo")
            .arg("/fo")
            .arg(&resource)
            .arg(&rc)
            .status()
            .expect("Run from a Windows SDK developer shell or set RC to rc.exe");
        assert!(status.success(), "Failed to compile application icon");
        println!("cargo:rustc-link-arg={}", resource.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    } else {
        let object = out.join("app.o");
        std::fs::write(
            &rc,
            format!(
                "{icon_resource}1 24 \"{}\"\n",
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
