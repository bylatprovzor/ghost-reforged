fn main() {
    println!("cargo:rerun-if-changed=design/ghost.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=src/native/audio.cpp");
        cc::Build::new()
            .cpp(true)
            .file("src/native/audio.cpp")
            .flag_if_supported("/std:c++17")
            .flag_if_supported("/EHsc")
            .compile("ghost_audio");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rerun-if-changed=src/native/audio_fixture.cpp");
        cc::Build::new()
            .cpp(true)
            .file("src/native/audio_fixture.cpp")
            .flag_if_supported("/std:c++17")
            .compile("ghost_audio_fixture");
        if std::env::var_os("CARGO_FEATURE_LAUNCHER").is_some() {
            println!("cargo:rerun-if-changed=src/native/video.cpp");
            cc::Build::new()
                .cpp(true)
                .file("src/native/video.cpp")
                .flag_if_supported("/std:c++17")
                .compile("ghost_video");
            for library in ["mfuuid", "ole32", "propsys"] {
                println!("cargo:rustc-link-lib={library}");
            }
        }
        winresource::WindowsResource::new()
            .set_icon("design/ghost.ico")
            .set("ProductName", "GHost Reforged")
            .set("FileDescription", "GHost Reforged")
            .set("OriginalFilename", "ghost-reforged.exe")
            .compile()
            .expect("Windows resources could not be compiled");
    }
}
