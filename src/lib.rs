//! Rustle filters, merges, and follows structured and plain-text logs.

mod cli;
mod console;
mod decode;
mod discover;
mod entry;
mod error;
mod filter;
mod follow;
mod level;
mod parse;
mod process;
mod render;
mod timeutil;

#[cfg(test)]
mod test_support;

pub use process::run;
