#![feature(phase)]

#[phase(plugin)]
extern crate clippy;

fn the_answer(ref mut x: u8) {
  *x = 42;
}

fn main() {
  let mut x = 0;
  the_answer(x);
  println!("The answer is {}.", x);
}
