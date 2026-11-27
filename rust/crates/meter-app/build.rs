fn main() {
    // Windows gives the main thread 1 MB of stack, less than Slint's component tree needs in a debug build: the meter
    // gets 8 MB, as Linux does by default.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        // pcap 2.5 brings windows-sys 0.36, whose windows.lib imports WaitOnAddress and WakeByAddress* from vertdll.dll
        // (the enclave runtime, which crashes outside an enclave). MSVC's linker takes each symbol from the first
        // library that has it, and this crate's own libraries come first: std's thread parking gets the real ones.
        println!("cargo:rustc-link-lib=dylib=synchronization");
    }
    // Debug builds keep the element names and accessibility data the UI tests and fixture screenshots look controls up by.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()).with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");
}
