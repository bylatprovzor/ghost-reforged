use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../..");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let archive = root.join(format!(
        "target/package/GHost-Reforged-{version}-windows-x64.zip"
    ));
    println!("cargo:rerun-if-changed={}", archive.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("design/ghost.ico").display()
    );
    std::fs::copy(
        archive,
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("payload.zip"),
    )
    .expect("Build the Windows ZIP with packaging/windows/build.ps1 first");
    winresource::WindowsResource::new()
        .set_icon(root.join("design/ghost.ico").to_str().unwrap())
        .set("ProductName", "GHost Reforged")
        .set("FileDescription", "GHost Reforged Setup")
        .set("OriginalFilename", "GHost-Reforged-Setup-x64.exe")
        .compile()
        .expect("Windows setup resources could not be compiled");
}
