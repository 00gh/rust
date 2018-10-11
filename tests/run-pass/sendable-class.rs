// Copyright 2012 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// FIXME validation disabled because ptr::read uses mem::uninitialized
// compile-flags: -Zmiri-disable-validation

// Test that a class with only sendable fields can be sent

use std::sync::mpsc::channel;

#[allow(dead_code)]
struct Foo {
    i: isize,
    j: char,
}

fn foo(i:isize, j: char) -> Foo {
    Foo {
        i: i,
        j: j
    }
}

pub fn main() {
    let (tx, rx) = channel();
    let _ = tx.send(foo(42, 'c'));
    let _ = rx;
}
