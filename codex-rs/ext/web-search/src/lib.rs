mod extension;
mod history;
mod output;
mod schema;
mod tinyfish;
mod tool;

pub use extension::install;

#[cfg(test)]
#[path = "tinyfish_tests.rs"]
mod tinyfish_tests;
