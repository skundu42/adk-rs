//! Reference `adk` binary. Without registered agents it serves as a help
//! shell — real applications should build their own binary on top of
//! `adk_rs_cli::App` and register agents directly.

fn main() -> adk_rs_error::Result<()> {
    let app = adk_rs_cli::App::new("adk-rs");
    app.run()
}
