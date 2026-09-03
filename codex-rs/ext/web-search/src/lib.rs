mod extension;
mod history;
mod output;
mod schema;
mod tinyfish_client;
mod tinyfish_output;
mod tinyfish_request;
mod tinyfish_tool;
mod tool;

pub use extension::install;
#[cfg(feature = "test-support")]
pub use extension::install_tinyfish_for_test;

#[cfg(test)]
#[path = "tinyfish_request_tests.rs"]
mod tinyfish_request_tests;

#[cfg(test)]
#[path = "tinyfish_output_tests.rs"]
mod tinyfish_output_tests;
#[cfg(test)]
#[path = "tinyfish_tool_tests.rs"]
mod tinyfish_tool_tests;
