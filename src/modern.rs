//! Versioned BFQ container using Argon2id and chunked AES-256-GCM.
//!
//! The original filename is encrypted as independent authenticated metadata.
//! Every data chunk and the mandatory final chunk authenticate the full header,
//! preventing undetected modification, reordering, and truncation.

use super::{
    Decoder, En3Error, MonitoredReader, Result, atomic_write, read_exact_context, safe_basename,
};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use argon2::{Algorithm, Argon2, Params, Version};
use sm3::{Digest, Sm3};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 8] = b"BFQPK01\0";
const FORMAT_VERSION: u16 = 1;
const KDF_ARGON2ID: u8 = 1;
const CIPHER_AES256GCM: u8 = 1;
const KEYFILE_FLAG: u16 = 0x01;
const FIXED_HEADER_SIZE: usize = 68;
const RECORD_HEADER_SIZE: usize = 13;
const TAG_SIZE: usize = 16;
const SALT_SIZE: usize = 16;
const NONCE_SIZE: usize = 12;
const MAX_FILENAME_SIZE: usize = 1024;
const FILENAME_NONCE_INDEX: u64 = u64::MAX;
const FILENAME_DOMAIN: &[u8] = b"BFQNAME1";
const ARGON_MEMORY_KIB: u32 = 19 * 1024;
const ARGON_ITERATIONS: u32 = 2;
const ARGON_LANES: u32 = 1;

pub const CHUNK_SIZE: usize = 64 * 1024;
pub const BFQ_EXTENSION: &str = "bfq";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub filename: String,
    pub original_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub filename: String,
    pub original_size: u64,
    pub output: PathBuf,
}

fn derive_key(
    password: &str,
    salt: &[u8; SALT_SIZE],
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    keyfile: Option<&[u8; 32]>,
) -> Result<Zeroizing<[u8; 32]>> {
    if password.is_empty() || password.len() > 1024 {
        return Err(En3Error::Format(
            "Modern password must contain 1..1024 UTF-8 bytes".into(),
        ));
    }
    if memory_kib != ARGON_MEMORY_KIB || iterations != ARGON_ITERATIONS || lanes != ARGON_LANES {
        return Err(En3Error::Format(
            "Unsupported Argon2id parameters for BFQ version 1".into(),
        ));
    }
    let params = Params::new(memory_kib, iterations, lanes, Some(32))
        .map_err(|error| En3Error::Format(format!("Invalid Argon2id parameters: {error}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; 32]);
    if let Some(digest) = keyfile {
        let mut hasher = Sm3::new();
        hasher.update(salt);
        hasher.update(digest);
        let mixed: [u8; 32] = hasher.finalize().into();
        argon2
            .hash_password_into(password.as_bytes(), &mixed, &mut *key)
            .map_err(|error| En3Error::Format(format!("Key derivation failed: {error}")))?;
    } else {
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut *key)
            .map_err(|error| En3Error::Format(format!("Key derivation failed: {error}")))?;
    }
    Ok(key)
}

/// Hash a key file into fixed-size key material. Any ordinary file works;
/// the file content itself is never stored, only its digest participates in
/// key derivation. Empty files are rejected.
pub fn load_keyfile(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let mut file = File::open(path)?;
    let mut hasher = Sm3::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        hasher.update(&buffer[..count]);
    }
    let mut buffer = Zeroizing::new(buffer);
    buffer.zeroize();
    if total == 0 {
        return Err(En3Error::Format("Key file is empty".into()));
    }
    Ok(Zeroizing::new(hasher.finalize().into()))
}

fn chunk_nonce(base_nonce: &[u8; NONCE_SIZE], index: u64) -> [u8; NONCE_SIZE] {
    let mut nonce = *base_nonce;
    let counter = index.to_be_bytes();
    for (byte, counter_byte) in nonce[4..].iter_mut().zip(counter) {
        *byte ^= counter_byte;
    }
    nonce
}

fn record_aad(header: &[u8], index: u64, flags: u8, ciphertext_size: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + RECORD_HEADER_SIZE);
    aad.extend_from_slice(header);
    aad.extend_from_slice(&index.to_le_bytes());
    aad.push(flags);
    aad.extend_from_slice(&ciphertext_size.to_le_bytes());
    aad
}

fn authentication_error() -> En3Error {
    En3Error::Format("Wrong password or corrupted BFQ archive (authentication failed)".into())
}

struct ModernEncoder<W: Write> {
    writer: W,
    cipher: Aes256Gcm,
    authenticated_header: Vec<u8>,
    base_nonce: [u8; NONCE_SIZE],
    buffer: Vec<u8>,
    chunk_index: u64,
    processed: u64,
    expected_size: u64,
    finished: bool,
}

impl<W: Write> ModernEncoder<W> {
    fn new(
        mut writer: W,
        password: &str,
        keyfile: Option<&[u8; 32]>,
        filename: &str,
        original_size: u64,
    ) -> Result<Self> {
        let safe_name = safe_basename(filename)?;
        if safe_name != filename {
            return Err(En3Error::Format(
                "BFQ input filename must be a safe basename".into(),
            ));
        }
        let filename_bytes = filename.as_bytes();
        if filename_bytes.len() > MAX_FILENAME_SIZE {
            return Err(En3Error::Format("BFQ filename is too long".into()));
        }

        let mut salt = [0_u8; SALT_SIZE];
        let mut base_nonce = [0_u8; NONCE_SIZE];
        getrandom::fill(&mut salt)
            .map_err(|error| En3Error::Format(format!("OS random source failed: {error}")))?;
        getrandom::fill(&mut base_nonce)
            .map_err(|error| En3Error::Format(format!("OS random source failed: {error}")))?;
        let key = derive_key(
            password,
            &salt,
            ARGON_MEMORY_KIB,
            ARGON_ITERATIONS,
            ARGON_LANES,
            keyfile,
        )?;
        let cipher = Aes256Gcm::new_from_slice(&*key)
            .map_err(|_| En3Error::Format("Internal AES-256 key error".into()))?;

        let encrypted_filename_size = (filename_bytes.len() + TAG_SIZE) as u16;
        let mut fixed_header = Vec::with_capacity(FIXED_HEADER_SIZE);
        fixed_header.extend_from_slice(MAGIC);
        fixed_header.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        fixed_header.push(KDF_ARGON2ID);
        fixed_header.push(CIPHER_AES256GCM);
        fixed_header.extend_from_slice(&(CHUNK_SIZE as u32).to_le_bytes());
        fixed_header.extend_from_slice(&original_size.to_le_bytes());
        fixed_header.extend_from_slice(&encrypted_filename_size.to_le_bytes());
        let flags = if keyfile.is_some() { KEYFILE_FLAG } else { 0 };
        fixed_header.extend_from_slice(&flags.to_le_bytes());
        fixed_header.extend_from_slice(&salt);
        fixed_header.extend_from_slice(&base_nonce);
        fixed_header.extend_from_slice(&ARGON_MEMORY_KIB.to_le_bytes());
        fixed_header.extend_from_slice(&ARGON_ITERATIONS.to_le_bytes());
        fixed_header.extend_from_slice(&ARGON_LANES.to_le_bytes());
        debug_assert_eq!(fixed_header.len(), FIXED_HEADER_SIZE);

        let mut filename_aad = fixed_header.clone();
        filename_aad.extend_from_slice(FILENAME_DOMAIN);
        let filename_nonce = chunk_nonce(&base_nonce, FILENAME_NONCE_INDEX);
        let encrypted_filename = cipher
            .encrypt(
                (&filename_nonce).into(),
                Payload {
                    msg: filename_bytes,
                    aad: &filename_aad,
                },
            )
            .map_err(|_| En3Error::Format("Filename encryption failed".into()))?;
        writer.write_all(&fixed_header)?;
        writer.write_all(&encrypted_filename)?;
        let mut authenticated_header = fixed_header;
        authenticated_header.extend_from_slice(&encrypted_filename);

        Ok(Self {
            writer,
            cipher,
            authenticated_header,
            base_nonce,
            buffer: Vec::with_capacity(CHUNK_SIZE),
            chunk_index: 0,
            processed: 0,
            expected_size: original_size,
            finished: false,
        })
    }

    fn append(&mut self, mut input: &[u8]) -> Result<()> {
        if self.finished {
            return Err(En3Error::Format(
                "Cannot append after BFQ final chunk".into(),
            ));
        }
        while !input.is_empty() {
            let available = CHUNK_SIZE - self.buffer.len();
            let count = available.min(input.len());
            self.buffer.extend_from_slice(&input[..count]);
            self.processed = self
                .processed
                .checked_add(count as u64)
                .ok_or_else(|| En3Error::Format("BFQ input size overflow".into()))?;
            if self.processed > self.expected_size {
                return Err(En3Error::Format(
                    "Input grew beyond the declared original size".into(),
                ));
            }
            input = &input[count..];
            if self.buffer.len() == CHUNK_SIZE {
                let chunk = std::mem::take(&mut self.buffer);
                self.seal_chunk(&chunk, false)?;
                self.buffer = Vec::with_capacity(CHUNK_SIZE);
            }
        }
        Ok(())
    }

    fn seal_chunk(&mut self, plaintext: &[u8], final_chunk: bool) -> Result<()> {
        let flags = u8::from(final_chunk);
        let ciphertext_size = (plaintext.len() + TAG_SIZE) as u32;
        let aad = record_aad(
            &self.authenticated_header,
            self.chunk_index,
            flags,
            ciphertext_size,
        );
        let nonce = chunk_nonce(&self.base_nonce, self.chunk_index);
        let ciphertext = self
            .cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| En3Error::Format("BFQ chunk encryption failed".into()))?;
        self.writer.write_all(&self.chunk_index.to_le_bytes())?;
        self.writer.write_all(&[flags])?;
        self.writer.write_all(&ciphertext_size.to_le_bytes())?;
        self.writer.write_all(&ciphertext)?;
        self.chunk_index = self
            .chunk_index
            .checked_add(1)
            .ok_or_else(|| En3Error::Format("Too many BFQ chunks".into()))?;
        if self.chunk_index == FILENAME_NONCE_INDEX {
            return Err(En3Error::Format("Too many BFQ chunks".into()));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        if self.processed != self.expected_size {
            return Err(En3Error::Format(
                "Input ended before the declared original size".into(),
            ));
        }
        let final_data = std::mem::take(&mut self.buffer);
        self.seal_chunk(&final_data, true)?;
        self.finished = true;
        self.writer.flush()?;
        Ok(())
    }
}

impl<W: Write> Write for ModernEncoder<W> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.append(input).map_err(io::Error::other)?;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

struct ModernDecoder<R: Read> {
    reader: R,
    cipher: Aes256Gcm,
    authenticated_header: Vec<u8>,
    base_nonce: [u8; NONCE_SIZE],
    filename: String,
    remaining: u64,
    original_size: u64,
    chunk_index: u64,
}

impl<R: Read> ModernDecoder<R> {
    fn new(mut reader: R, password: &str, keyfile: Option<&[u8; 32]>) -> Result<Self> {
        let mut fixed = [0_u8; FIXED_HEADER_SIZE];
        read_exact_context(&mut reader, &mut fixed, "Truncated BFQ header")?;
        if &fixed[..8] != MAGIC {
            return Err(En3Error::Format("Not a BFQ archive".into()));
        }
        let version = u16::from_le_bytes(fixed[8..10].try_into().expect("version"));
        if version != FORMAT_VERSION {
            return Err(En3Error::Format(format!(
                "Unsupported BFQ version: {version}"
            )));
        }
        if fixed[10] != KDF_ARGON2ID || fixed[11] != CIPHER_AES256GCM {
            return Err(En3Error::Format("Unsupported BFQ algorithms".into()));
        }
        let chunk_size = u32::from_le_bytes(fixed[12..16].try_into().expect("chunk size"));
        if chunk_size != CHUNK_SIZE as u32 {
            return Err(En3Error::Format("Unsupported BFQ chunk size".into()));
        }
        let original_size = u64::from_le_bytes(fixed[16..24].try_into().expect("original size"));
        let filename_size =
            u16::from_le_bytes(fixed[24..26].try_into().expect("filename size")) as usize;
        if !(TAG_SIZE..=MAX_FILENAME_SIZE + TAG_SIZE).contains(&filename_size) {
            return Err(En3Error::Format("Invalid BFQ filename length".into()));
        }
        let flags = u16::from_le_bytes(fixed[26..28].try_into().expect("flags"));
        if flags & !KEYFILE_FLAG != 0 {
            return Err(En3Error::Format("Unsupported BFQ header flags".into()));
        }
        let needs_keyfile = flags & KEYFILE_FLAG != 0;
        match (needs_keyfile, keyfile.is_some()) {
            (true, false) => {
                return Err(En3Error::Format(
                    "This archive requires a key file (key file missing)".into(),
                ));
            }
            (false, true) => {
                return Err(En3Error::Format(
                    "This archive was created without a key file".into(),
                ));
            }
            _ => {}
        }
        let salt: [u8; SALT_SIZE] = fixed[28..44].try_into().expect("salt");
        let base_nonce: [u8; NONCE_SIZE] = fixed[44..56].try_into().expect("nonce");
        let memory_kib = u32::from_le_bytes(fixed[56..60].try_into().expect("memory cost"));
        let iterations = u32::from_le_bytes(fixed[60..64].try_into().expect("time cost"));
        let lanes = u32::from_le_bytes(fixed[64..68].try_into().expect("lanes"));
        let key = derive_key(password, &salt, memory_kib, iterations, lanes, keyfile)?;
        let cipher = Aes256Gcm::new_from_slice(&*key)
            .map_err(|_| En3Error::Format("Internal AES-256 key error".into()))?;

        let mut encrypted_filename = vec![0_u8; filename_size];
        read_exact_context(
            &mut reader,
            &mut encrypted_filename,
            "Truncated BFQ filename",
        )?;
        let mut filename_aad = fixed.to_vec();
        filename_aad.extend_from_slice(FILENAME_DOMAIN);
        let filename_nonce = chunk_nonce(&base_nonce, FILENAME_NONCE_INDEX);
        let filename_bytes = cipher
            .decrypt(
                (&filename_nonce).into(),
                Payload {
                    msg: &encrypted_filename,
                    aad: &filename_aad,
                },
            )
            .map_err(|_| authentication_error())?;
        let decoded_name = String::from_utf8(filename_bytes)
            .map_err(|_| En3Error::Format("Invalid UTF-8 BFQ filename".into()))?;
        let filename = safe_basename(&decoded_name)?;
        if filename != decoded_name {
            return Err(En3Error::Format("Unsafe BFQ archived filename".into()));
        }
        let mut authenticated_header = fixed.to_vec();
        authenticated_header.extend_from_slice(&encrypted_filename);

        Ok(Self {
            reader,
            cipher,
            authenticated_header,
            base_nonce,
            filename,
            remaining: original_size,
            original_size,
            chunk_index: 0,
        })
    }

    fn decode_to<W: Write>(mut self, writer: &mut W) -> Result<StreamInfo> {
        loop {
            let mut record = [0_u8; RECORD_HEADER_SIZE];
            read_exact_context(&mut self.reader, &mut record, "Truncated BFQ record header")?;
            let index = u64::from_le_bytes(record[..8].try_into().expect("chunk index"));
            let flags = record[8];
            let ciphertext_size =
                u32::from_le_bytes(record[9..13].try_into().expect("ciphertext size")) as usize;
            if index != self.chunk_index || flags > 1 {
                return Err(En3Error::Format("Unexpected BFQ chunk sequence".into()));
            }
            if !(TAG_SIZE..=CHUNK_SIZE + TAG_SIZE).contains(&ciphertext_size) {
                return Err(En3Error::Format("Invalid BFQ ciphertext length".into()));
            }
            let plaintext_size = ciphertext_size - TAG_SIZE;
            let final_chunk = flags == 1;
            if (!final_chunk && plaintext_size != CHUNK_SIZE)
                || (final_chunk && plaintext_size as u64 != self.remaining)
                || plaintext_size as u64 > self.remaining
            {
                return Err(En3Error::Format(
                    "Invalid BFQ chunk size or final marker".into(),
                ));
            }
            let mut ciphertext = vec![0_u8; ciphertext_size];
            read_exact_context(&mut self.reader, &mut ciphertext, "Truncated BFQ chunk")?;
            let aad = record_aad(
                &self.authenticated_header,
                index,
                flags,
                ciphertext_size as u32,
            );
            let nonce = chunk_nonce(&self.base_nonce, index);
            let plaintext = self
                .cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| authentication_error())?;
            writer.write_all(&plaintext)?;
            self.remaining -= plaintext_size as u64;
            self.chunk_index = self
                .chunk_index
                .checked_add(1)
                .ok_or_else(|| En3Error::Format("Too many BFQ chunks".into()))?;
            if final_chunk {
                break;
            }
        }
        let mut trailing = [0_u8; 1];
        if self.reader.read(&mut trailing)? != 0 {
            return Err(En3Error::Format("BFQ archive has trailing bytes".into()));
        }
        writer.flush()?;
        Ok(StreamInfo {
            filename: self.filename,
            original_size: self.original_size,
        })
    }
}

/// Encrypt a stream into BFQ without loading the complete input into memory.
pub fn encrypt<R: Read, W: Write>(
    mut reader: R,
    writer: W,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    filename: &str,
    original_size: u64,
) -> Result<StreamInfo> {
    let mut encoder = ModernEncoder::new(writer, password, keyfile, filename, original_size)?;
    io::copy(&mut reader, &mut encoder)?;
    encoder.finish()?;
    Ok(StreamInfo {
        filename: filename.to_string(),
        original_size,
    })
}

/// Decrypt and authenticate a BFQ stream incrementally.
pub fn decrypt<R: Read, W: Write>(
    reader: R,
    writer: &mut W,
    password: &str,
    keyfile: Option<&[u8; 32]>,
) -> Result<StreamInfo> {
    ModernDecoder::new(reader, password, keyfile)?.decode_to(writer)
}

/// Resolve an output collision without ever replacing an existing file.
pub fn choose_output(output: &Path, rename_existing: bool) -> Result<PathBuf> {
    if !output.exists() {
        return Ok(output.to_path_buf());
    }
    if !rename_existing {
        return Err(En3Error::OutputExists(output.to_path_buf()));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| En3Error::Format("Output filename must be valid Unicode".into()))?;
    let extension = output.extension().and_then(|value| value.to_str());
    for index in 1..100_000_u32 {
        let filename = match extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        let candidate = parent.join(filename);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(En3Error::Format(
        "Unable to find a free output filename".into(),
    ))
}

pub fn inspect_file(
    archive: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
) -> Result<StreamInfo> {
    let decoder = ModernDecoder::new(File::open(archive)?, password, keyfile)?;
    Ok(StreamInfo {
        filename: decoder.filename,
        original_size: decoder.original_size,
    })
}

pub fn verify_file_monitored<F: FnMut(u64) -> io::Result<()>>(
    archive: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    on_progress: F,
) -> Result<FileInfo> {
    let reader = MonitoredReader::new(File::open(archive)?, on_progress);
    let info = decrypt(reader, &mut io::sink(), password, keyfile)?;
    Ok(FileInfo {
        filename: info.filename,
        original_size: info.original_size,
        output: PathBuf::new(),
    })
}

pub fn encrypt_file(
    input: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
) -> Result<FileInfo> {
    encrypt_file_monitored(input, password, keyfile, output, |_| Ok(()))
}

pub fn encrypt_file_monitored<F: FnMut(u64) -> io::Result<()>>(
    input: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
    on_progress: F,
) -> Result<FileInfo> {
    let original_size = input.metadata()?.len();
    let filename = input
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| En3Error::Format("Input filename must be valid Unicode".into()))?
        .to_string();
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| input.with_extension(BFQ_EXTENSION));
    let reader = MonitoredReader::new(File::open(input)?, on_progress);
    atomic_write(&output, |file| {
        encrypt(reader, file, password, keyfile, &filename, original_size)
    })?;
    Ok(FileInfo {
        filename,
        original_size,
        output,
    })
}

pub fn decrypt_file(
    archive: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
) -> Result<FileInfo> {
    decrypt_file_monitored(archive, password, keyfile, output, |_| Ok(()))
}

pub fn decrypt_file_monitored<F: FnMut(u64) -> io::Result<()>>(
    archive: &Path,
    password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
    on_progress: F,
) -> Result<FileInfo> {
    let reader = MonitoredReader::new(File::open(archive)?, on_progress);
    let decoder = ModernDecoder::new(reader, password, keyfile)?;
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| archive.with_file_name(decoder.filename.clone()));
    let filename = decoder.filename.clone();
    let original_size = decoder.original_size;
    atomic_write(&output, |file| decoder.decode_to(file))?;
    Ok(FileInfo {
        filename,
        original_size,
        output,
    })
}

/// Migrate EN3 directly to BFQ without creating a plaintext intermediate file.
pub fn migrate_en3(
    archive: &Path,
    old_password: &str,
    new_password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
) -> Result<FileInfo> {
    migrate_en3_monitored(archive, old_password, new_password, keyfile, output, |_| Ok(()))
}

pub fn migrate_en3_monitored<F: FnMut(u64) -> io::Result<()>>(
    archive: &Path,
    old_password: &str,
    new_password: &str,
    keyfile: Option<&[u8; 32]>,
    output: Option<&Path>,
    on_progress: F,
) -> Result<FileInfo> {
    let reader = MonitoredReader::new(File::open(archive)?, on_progress);
    let decoder = Decoder::new(reader, old_password)?;
    let filename = decoder.filename.clone();
    let original_size = decoder.original_size;
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| archive.with_extension(BFQ_EXTENSION));
    atomic_write(&output, |file| {
        let mut encoder = ModernEncoder::new(file, new_password, keyfile, &filename, original_size)?;
        decoder.decode_to(&mut encoder)?;
        encoder.finish()
    })?;
    Ok(FileInfo {
        filename,
        original_size,
        output,
    })
}
