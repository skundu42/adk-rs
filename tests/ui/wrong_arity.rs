#![allow(dead_code)]

struct Args {
    x: i32,
}

#[adk_rs::tool]
/// Missing the ctx argument.
async fn nope(_args: Args) -> adk_rs::Result<i32> {
    Ok(1)
}

fn main() {}
