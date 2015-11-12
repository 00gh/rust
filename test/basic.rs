#![feature(custom_attribute)]
#![allow(dead_code, unused_attributes)]

#[miri_run(expected = "Int(1)")]
fn ret() -> i32 {
    1
}

#[miri_run(expected = "Int(3)")]
fn add() -> i32 {
    1 + 2
}

#[miri_run(expected = "Int(3)")]
fn indirect_add() -> i32 {
    let x = 1;
    let y = 2;
    x + y
}

fn main() {}
