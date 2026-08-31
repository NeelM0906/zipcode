use super::create_archive;
use super::file_sha256;
use super::hex_digest;
use super::millis_rfc3339;
use flate2::read::GzDecoder;
use pretty_assertions::assert_eq;
use std::io::Read;
use std::io::Write;

#[test]
fn archive_contains_complete_bundle() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let bundle = directory.path().join("bundle");
    std::fs::create_dir(&bundle).expect("create bundle");
    std::fs::write(bundle.join("manifest.json"), b"manifest").expect("write manifest");
    std::fs::write(bundle.join("events.jsonl"), b"event\n").expect("write events");
    let destination = directory.path().join("bundle.tar.gz");

    create_archive(&bundle, &destination).expect("create archive");

    let decoder = GzDecoder::new(std::fs::File::open(destination).expect("open archive"));
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    for entry in archive.entries().expect("archive entries") {
        let mut entry = entry.expect("archive entry");
        if entry.header().entry_type().is_file() {
            let mut contents = String::new();
            entry.read_to_string(&mut contents).expect("read entry");
            files.push((
                entry
                    .path()
                    .expect("entry path")
                    .to_string_lossy()
                    .into_owned(),
                contents,
            ));
        }
    }
    files.sort();
    assert_eq!(
        files,
        vec![
            ("trace/events.jsonl".to_string(), "event\n".to_string()),
            ("trace/manifest.json".to_string(), "manifest".to_string()),
        ]
    );
}

#[test]
fn hashes_parts_and_files_identically() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("part.bin");
    let bytes = b"complete ZIPCODE rollout evidence";
    std::fs::File::create(&path)
        .expect("create part")
        .write_all(bytes)
        .expect("write part");
    assert_eq!(
        file_sha256(&path).expect("hash file"),
        (hex_digest(bytes), bytes.len() as u64)
    );
}

#[test]
fn renders_trace_timestamp_as_rfc3339() {
    assert_eq!(
        millis_rfc3339(1_788_024_600_123).expect("valid timestamp"),
        "2026-08-29T17:30:00.123Z"
    );
}
