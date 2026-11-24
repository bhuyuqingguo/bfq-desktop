use en3_recovery::{En3Error, decode};
use std::io::{Cursor, Read};

const KNOWN_PLAINTEXT: &[u8] = include_bytes!("fixtures/known_plaintext.bin");
const MULTIBLOCK_PLAINTEXT: &[u8] = include_bytes!("fixtures/multiblock_plaintext.bin");
const SAMPLE1: &[u8] = include_bytes!("fixtures/sample1.en3");
const SAMPLE2: &[u8] = include_bytes!("fixtures/sample2.en3");
const MULTIBLOCK: &[u8] = include_bytes!("fixtures/multiblock_plaintext.en3");

fn run_decode(archive: &[u8], password: &str) -> Result<(String, Vec<u8>), En3Error> {
    let mut plaintext = Vec::new();
    let info = decode(Cursor::new(archive), &mut plaintext, password)?;
    Ok((info.filename, plaintext))
}

#[test]
fn single_block_samples_match_original_files() {
    for (archive, password) in [
        (SAMPLE1, "指一在必制长文常位坚"),
        (SAMPLE2, "标主挥这政新的好主安"),
    ] {
        let (filename, plaintext) = run_decode(archive, password).expect("valid original sample");
        assert_eq!(filename, "known_plaintext.bin");
        assert_eq!(plaintext, KNOWN_PLAINTEXT);
    }
}

#[test]
fn three_block_sample_matches_original_file() {
    let (filename, plaintext) =
        run_decode(MULTIBLOCK, "要U夕生和出大发8出的").expect("valid original three-block sample");
    assert_eq!(filename, "multiblock_plaintext.bin");
    assert_eq!(plaintext.len(), 10_000);
    assert_eq!(plaintext, MULTIBLOCK_PLAINTEXT);
}

#[test]
fn wrong_password_is_rejected() {
    assert!(run_decode(SAMPLE2, "指一在必制长文常位坚").is_err());
}

#[test]
fn tampered_data_is_rejected() {
    let mut archive = SAMPLE2.to_vec();
    archive[100] ^= 1;
    let error = run_decode(&archive, "标主挥这政新的好主安").unwrap_err();
    assert!(
        error.to_string().contains("checksum"),
        "unexpected error: {error}"
    );
}

#[test]
fn truncated_archive_is_rejected() {
    let error = run_decode(&SAMPLE2[..SAMPLE2.len() - 1], "标主挥这政新的好主安").unwrap_err();
    assert!(
        error.to_string().contains("Truncated"),
        "unexpected error: {error}"
    );
}

struct TinyReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Read for TinyReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.position == self.bytes.len() {
            return Ok(0);
        }
        let count = output.len().min(7).min(self.bytes.len() - self.position);
        output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[test]
fn decoding_works_with_fragmented_stream_reads() {
    let reader = TinyReader {
        bytes: MULTIBLOCK,
        position: 0,
    };
    let mut plaintext = Vec::new();
    let info = decode(reader, &mut plaintext, "要U夕生和出大发8出的")
        .expect("decoder must not assume full-buffer reads");
    assert_eq!(info.original_size, 10_000);
    assert_eq!(plaintext, MULTIBLOCK_PLAINTEXT);
}
