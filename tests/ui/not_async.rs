#![allow(dead_code)]

use adk_rs::core::ToolContext;

struct Args {
    x: i32,
}

#[adk_rs::tool]
/// Not async.
fn nope(_args: Args, _ctx: &mut ToolContext) -> adk_rs::Result<i32> {
    Ok(1)
}

fn main() {}
