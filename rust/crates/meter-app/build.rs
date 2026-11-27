fn main() {
    // Debug builds keep the element names and accessibility data the UI tests and fixture screenshots look controls up by.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()).with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compiles");
}
