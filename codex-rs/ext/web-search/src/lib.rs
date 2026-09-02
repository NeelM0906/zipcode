mod extension;
mod history;
mod output;
mod schema;
mod tinyfish_request;
mod tool;

pub use extension::install;

#[cfg(test)]
#[path = "tinyfish_request_tests.rs"]
mod tinyfish_request_tests;
