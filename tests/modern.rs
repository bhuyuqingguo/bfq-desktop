use en3_recovery::modern::{CHUNK_SIZE, decrypt, decrypt_file, encrypt, migrate_en3};
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_data(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| ((index * 131 + index / 251 + 17) & 0xff) as u8)
        .collect()
}

fn encrypt_bytes(data: &[u8], password: &str) -> Vec<u8> {
    let mut archive = Vec::new();
    encrypt(
        Cursor::new(data),
        &mut archive,
        password,
        "secret-name.bin",
        data.len() as u64,
    )
    .expect("encrypt test data");
    archive
}

fn decrypt_bytes(archive: &[u8], password: &str) -> Result<(String, Vec<u8>), String> {
    let mut plaintext = Vec::new();
    let info = decrypt(Cursor::new(archive), &mut plaintext, password)
        .map_err(|error| error.to_string())?;
    Ok((info.filename, plaintext))
}

#[test]
fn roundtrip_covers_multiple_chunks_and_encrypted_filename() {
    let expected = test_data(200_000);
    let archive = encrypt_bytes(&expected, "correct horse battery staple");
    assert!(
        archive
            .windows(b"secret-name.bin".len())
            .all(|window| window != b"secret-name.bin")
    );
    let (filename, plaintext) = decrypt_bytes(&archive, "correct horse battery staple").unwrap();
    assert_eq!(filename, "secret-name.bin");
    assert_eq!(plaintext, expected);
}

#[test]
fn empty_and_exact_chunk_boundary_roundtrip() {
    for size in [0, CHUNK_SIZE, CHUNK_SIZE * 2] {
        let expected = test_data(size);
        let archive = encrypt_bytes(&expected, "口令-123");
        let (_, plaintext) = decrypt_bytes(&archive, "口令-123").unwrap();
        assert_eq!(plaintext, expected);
    }
}

#[test]
fn same_input_uses_fresh_random_salt_and_nonce() {
    let data = test_data(4096);
    let first = encrypt_bytes(&data, "same-password");
    let second = encrypt_bytes(&data, "same-password");
    assert_ne!(first, second);
    assert_eq!(decrypt_bytes(&first, "same-password").unwrap().1, data);
    assert_eq!(decrypt_bytes(&second, "same-password").unwrap().1, data);
}

#[test]
fn wrong_password_is_rejected() {
    let archive = encrypt_bytes(&test_data(1024), "right-password");
    let error = decrypt_bytes(&archive, "wrong-password").unwrap_err();
    assert!(error.contains("authentication failed"), "{error}");
}

#[test]
fn ciphertext_and_header_tampering_are_rejected() {
    let mut archive = encrypt_bytes(&test_data(70_000), "password");
    archive[40] ^= 1;
    assert!(decrypt_bytes(&archive, "password").is_err());

    let mut archive = encrypt_bytes(&test_data(70_000), "password");
    let last = archive.len() - 1;
    archive[last] ^= 1;
    let error = decrypt_bytes(&archive, "password").unwrap_err();
    assert!(error.contains("authentication failed"), "{error}");
}

#[test]
fn chunk_sequence_and_final_marker_tampering_are_rejected() {
    let mut archive = encrypt_bytes(&test_data(70_000), "password");
    let encrypted_filename_size = u16::from_le_bytes([archive[24], archive[25]]) as usize;
    let record = 68 + encrypted_filename_size;
    archive[record] ^= 1;
    let error = decrypt_bytes(&archive, "password").unwrap_err();
    assert!(error.contains("sequence"), "{error}");

    let mut archive = encrypt_bytes(&test_data(70_000), "password");
    let encrypted_filename_size = u16::from_le_bytes([archive[24], archive[25]]) as usize;
    let record = 68 + encrypted_filename_size;
    archive[record + 8] = 1;
    assert!(decrypt_bytes(&archive, "password").is_err());
}

#[test]
fn truncation_and_trailing_data_are_rejected() {
    let archive = encrypt_bytes(&test_data(70_000), "password");
    let error = decrypt_bytes(&archive[..archive.len() - 1], "password").unwrap_err();
    assert!(error.contains("Truncated"), "{error}");

    let mut extended = archive;
    extended.push(0);
    let error = decrypt_bytes(&extended, "password").unwrap_err();
    assert!(error.contains("trailing"), "{error}");
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
        let count = output.len().min(11).min(self.bytes.len() - self.position);
        output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[test]
fn fragmented_input_and_archive_reads_are_supported() {
    let expected = test_data(CHUNK_SIZE + 123);
    let reader = TinyReader {
        bytes: &expected,
        position: 0,
    };
    let mut archive = Vec::new();
    encrypt(
        reader,
        &mut archive,
        "password",
        "secret-name.bin",
        expected.len() as u64,
    )
    .unwrap();

    let reader = TinyReader {
        bytes: &archive,
        position: 0,
    };
    let mut plaintext = Vec::new();
    decrypt(reader, &mut plaintext, "password").unwrap();
    assert_eq!(plaintext, expected);
}

fn test_output_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = PathBuf::from("target")
        .join("test-output")
        .join(format!("{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn en3_migrates_directly_to_authenticated_bfq() {
    let directory = test_output_dir();
    let archive = directory.join("migrated.bfq");
    let recovered = directory.join("recovered.bin");
    migrate_en3(
        PathBuf::from("tests/fixtures/multiblock_plaintext.en3").as_path(),
        "要U夕生和出大发8出的",
        "new-strong-password",
        Some(&archive),
    )
    .unwrap();
    decrypt_file(&archive, "new-strong-password", Some(&recovered)).unwrap();
    assert_eq!(
        fs::read(&recovered).unwrap(),
        fs::read("tests/fixtures/multiblock_plaintext.bin").unwrap()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_file_decryption_leaves_no_output() {
    let directory = test_output_dir();
    let archive = directory.join("sample.bfq");
    let recovered = directory.join("recovered.bin");
    fs::write(&archive, encrypt_bytes(&test_data(4096), "right-password")).unwrap();
    assert!(decrypt_file(&archive, "wrong-password", Some(&recovered)).is_err());
    assert!(!recovered.exists());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}
