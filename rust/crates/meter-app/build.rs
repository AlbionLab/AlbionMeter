fn main() {
    // Windows gives the main thread 1 MB of stack, less than Slint's component tree needs in a debug build: the meter
    // gets 8 MB, as Linux does by default.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        // pcap 2.5 brings windows-sys 0.36, whose windows.lib imports WaitOnAddress and WakeByAddress* from vertdll.dll
        // (the enclave runtime, which crashes outside an enclave). Linking synchronization.lib alone fixed the release
        // build (LTO) but not a debug one, where std still found windows.lib first: the symbols are made unresolved
        // from the start of the link instead, so they come from this crate's synchronization.lib, listed first.
        println!("cargo:rustc-link-lib=dylib=synchronization");
        for symbol in ["WaitOnAddress", "WakeByAddressSingle", "WakeByAddressAll"] {
            println!("cargo:rustc-link-arg=/INCLUDE:__imp_{symbol}");
        }
    }
    // What Windows shows for the exe (Task Manager, Explorer, the taskbar): the name Albion Meter and the AlbionLab logo,
    // instead of the file name and a blank icon.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("FileDescription", "Albion Meter")
            .set("ProductName", "Albion Meter")
            .set("CompanyName", "AlbionLab")
            .set("LegalCopyright", "AlbionLab")
            .set("OriginalFilename", "meter-app.exe");
        resource.compile().expect("Windows resources compile");
    }
    // Debug builds keep the element names and accessibility data the UI tests and fixture screenshots look controls up by.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()).with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");
}
