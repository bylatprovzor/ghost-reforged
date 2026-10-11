fn main() {
    println!("cargo:rerun-if-changed=design/ghost.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("design/ghost.ico")
            .set("ProductName", "GHost Reforged")
            .set("FileDescription", "GHost Reforged")
            .set("OriginalFilename", "ghost-reforged.exe")
            .compile()
            .expect("Windows resources could not be compiled");
    }
}
