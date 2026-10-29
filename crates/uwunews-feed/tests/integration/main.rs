//! Parsing and OPML against fixtures. No network: discovery's page parsing is
//! tested on a canned answer.

mod discover;
mod opml;
mod parse;

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
