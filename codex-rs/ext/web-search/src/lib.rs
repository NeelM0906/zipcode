mod extension;
mod history;
mod output;
mod schema;
mod tinyfish;
mod tool;

pub use extension::install;
#[cfg(feature = "test-support")]
pub use extension::install_tinyfish_for_test;

#[cfg(test)]
#[path = "tinyfish_tests.rs"]
mod tinyfish_tests;
