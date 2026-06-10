#![allow(dead_code)]

use adk_rs::core::ToolContext;

struct Args {
    x: i32,
}

#[adk_rs::tool]
/// First arg must be a simple identifier.
async fn nope(Args { x }: Args, _ctx: &mut ToolContext) -> adk_rs::Result<i32> {
    Ok(x)
}

fn main() {}
