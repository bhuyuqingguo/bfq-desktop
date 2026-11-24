//! Read-only decoder for the legacy EN3 format recovered from V0.7.
//!
//! This module exists only for recovery and migration. It deliberately does not
//! implement EN3 creation: the legacy format lacks modern authenticated encryption.

pub mod modern;

use encoding_rs::GB18030;
use sm3::{Digest, Sm3};
use sm4::Sm4;
use sm4::cipher::{BlockCipherDecrypt, KeyInit};
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MODULUS: i64 = 2_147_483_647;
const RAND_SCALE: f64 = 4.656_612_879_582_606e-10;
const MAX_BLOCK_PLAINTEXT: usize = 4095;

#[derive(Debug)]
pub enum En3Error {
    Io(io::Error),
    Format(String),
    OutputExists(PathBuf),
}

impl fmt::Display for En3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Format(message) => f.write_str(message),
            Self::OutputExists(path) => {
                write!(
                    f,
                    "Output already exists; refusing to overwrite: {}",
                    path.display()
                )
            }
        }
    }
}

impl Error for En3Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for En3Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, En3Error>;

/// A streaming reader that reports consumed input bytes and can be cancelled by
/// returning an I/O error from the callback.
pub struct MonitoredReader<R, F> {
    inner: R,
    processed: u64,
    on_progress: F,
}

impl<R, F> MonitoredReader<R, F> {
    pub fn new(inner: R, on_progress: F) -> Self {
        Self {
            inner,
            processed: 0,
            on_progress,
        }
    }
}

fn progress_error(error: io::Error) -> io::Error {
    // `read_exact` retries Interrupted errors automatically. Cancellation must
    // instead surface as a non-retryable error or it can loop forever.
    if error.kind() == io::ErrorKind::Interrupted {
        io::Error::other(error)
    } else {
        error
    }
}

impl<R: Read, F: FnMut(u64) -> io::Result<()>> Read for MonitoredReader<R, F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        (self.on_progress)(self.processed).map_err(progress_error)?;
        let count = self.inner.read(output)?;
        self.processed = self
            .processed
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("input byte counter overflow"))?;
        (self.on_progress)(self.processed).map_err(progress_error)?;
        Ok(count)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeInfo {
    pub filename: String,
    pub original_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInfo {
    pub filename: String,
    pub original_size: u64,
    pub output: PathBuf,
}

fn legacy_digest(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sm3::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn sm3_chain_xor(buffer: &mut [u8], password: &[u8]) {
    let mut digest = legacy_digest(password);
    for chunk in buffer.chunks_mut(32) {
        for (byte, mask) in chunk.iter_mut().zip(digest.iter()) {
            *byte ^= mask;
        }
        let mut hasher = Sm3::new();
        hasher.update(digest);
        hasher.update(password);
        digest = hasher.finalize().into();
    }
}

struct LegacyParkMiller {
    state: i32,
}

impl LegacyParkMiller {
    fn new(seed: u32, password: &[u8]) -> Self {
        let digest = legacy_digest(password);
        let digest_word =
            u32::from_le_bytes(digest[..4].try_into().expect("four-byte digest word"));
        Self {
            state: (seed ^ digest_word) as i32,
        }
    }

    fn next_state(&mut self) -> i32 {
        let product = 16_807_i64 * i64::from(self.state);
        self.state = (product % MODULUS) as i32;
        self.state.wrapping_sub(1)
    }

    fn rand(&mut self, maximum: i32) -> i32 {
        let unit = f64::from(self.next_state()) * RAND_SCALE;
        let value = (unit * f64::from(maximum + 1)).floor() as i32;
        if value == maximum + 1 { maximum } else { value }
    }

    fn fill_bytes(&mut self, output: &mut [u8]) {
        for byte in output {
            *byte = (self.rand(0x4000_0000) & 0xff) as u8;
        }
    }
}

fn decrypt_ecb(ciphertext: &mut [u8], key: &[u8]) -> Result<()> {
    if !ciphertext.len().is_multiple_of(16) {
        return Err(En3Error::Format(
            "SM4 ciphertext length is not a multiple of 16".into(),
        ));
    }
    let cipher = Sm4::new_from_slice(key)
        .map_err(|_| En3Error::Format("Internal error: invalid SM4 key length".into()))?;
    let (blocks, remainder) = ciphertext.as_chunks_mut::<16>();
    debug_assert!(remainder.is_empty());
    for block in blocks {
        cipher.decrypt_block(block.into());
    }
    Ok(())
}

fn read_exact_context<R: Read>(reader: &mut R, buffer: &mut [u8], message: &str) -> Result<()> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            En3Error::Format(message.into())
        } else {
            En3Error::Io(error)
        }
    })
}

fn safe_basename(decoded_name: &str) -> Result<String> {
    let basename = decoded_name
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_string();
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.chars().any(|character| {
            character.is_control() || matches!(character, '<' | '>' | ':' | '|' | '?' | '*')
        })
    {
        return Err(En3Error::Format(
            "Wrong password or corrupted archive (unsafe filename)".into(),
        ));
    }
    Ok(basename)
}

struct Decoder<R: Read> {
    reader: R,
    password: Vec<u8>,
    key_material: [u8; 64],
    mask: Vec<u8>,
    filename: String,
    remaining: u64,
    original_size: u64,
    block_index: u64,
}

impl<R: Read> Decoder<R> {
    fn new(mut reader: R, password_text: &str) -> Result<Self> {
        let mut header = [0_u8; 24];
        read_exact_context(&mut reader, &mut header, "Truncated enc3 header")?;
        if &header[..4] != b"enc3" {
            return Err(En3Error::Format("Not an enc3 archive".into()));
        }

        let original_size = u64::from_le_bytes(header[4..12].try_into().expect("eight-byte size"));
        let seed = u32::from_le_bytes(header[12..16].try_into().expect("four-byte seed"));
        let marker = u32::from_le_bytes(header[16..20].try_into().expect("four-byte marker"));
        let filename_size =
            u32::from_le_bytes(header[20..24].try_into().expect("four-byte filename size"))
                as usize;
        if marker != 0x0fff {
            return Err(En3Error::Format(format!("Unexpected marker: {marker:#x}")));
        }
        if !(16..=256).contains(&filename_size) || !filename_size.is_multiple_of(16) {
            return Err(En3Error::Format("Invalid encrypted filename length".into()));
        }

        let (password, _, password_errors) = GB18030.encode(password_text);
        if password_errors {
            return Err(En3Error::Format(
                "Legacy password contains characters unavailable in GB18030".into(),
            ));
        }
        let password = password.into_owned();
        if !(6..=32).contains(&password.len()) {
            return Err(En3Error::Format(
                "Legacy password must occupy 6..32 GBK bytes".into(),
            ));
        }

        let mut key_material = [0_u8; 64];
        sm3_chain_xor(&mut key_material, &password);
        let mut mask = vec![0_u8; 4096];
        LegacyParkMiller::new(seed, &password).fill_bytes(&mut mask);
        sm3_chain_xor(&mut mask, &password);

        let mut filename_plaintext = vec![0_u8; filename_size];
        read_exact_context(
            &mut reader,
            &mut filename_plaintext,
            "Truncated filename block",
        )?;
        decrypt_ecb(&mut filename_plaintext, &key_material[..16])?;
        for (byte, mask_byte) in filename_plaintext.iter_mut().zip(mask.iter()) {
            *byte ^= mask_byte;
        }
        let filename_end = filename_plaintext
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(filename_plaintext.len());
        let (decoded_name, _, filename_errors) =
            GB18030.decode(&filename_plaintext[..filename_end]);
        if filename_errors {
            return Err(En3Error::Format(
                "Wrong password or corrupted archive (invalid filename)".into(),
            ));
        }
        let filename = safe_basename(&decoded_name)?;

        Ok(Self {
            reader,
            password,
            key_material,
            mask,
            filename,
            remaining: original_size,
            original_size,
            block_index: 1,
        })
    }

    fn decode_to<W: Write>(mut self, writer: &mut W) -> Result<DecodeInfo> {
        while self.remaining != 0 {
            let plaintext_size = usize::try_from(self.remaining.min(MAX_BLOCK_PLAINTEXT as u64))
                .expect("block size always fits usize");
            let ciphertext_size = if plaintext_size == MAX_BLOCK_PLAINTEXT {
                4096
            } else {
                ((plaintext_size + 1) / 16 + 1) * 16
            };
            let mut block = vec![0_u8; ciphertext_size];
            read_exact_context(&mut self.reader, &mut block, "Truncated file-data block")?;

            let key_offset = (self.block_index % 49) as usize;
            decrypt_ecb(&mut block, &self.key_material[key_offset..key_offset + 16])?;
            sm3_chain_xor(&mut self.mask, &self.password);
            for (byte, mask_byte) in block.iter_mut().zip(self.mask.iter()) {
                *byte ^= mask_byte;
            }

            let content = &block[..plaintext_size];
            let checksum = content.iter().fold(0xf5_u8, |sum, byte| sum ^ byte);
            if block[plaintext_size] != checksum {
                return Err(En3Error::Format(
                    "Wrong password or corrupted archive (XOR checksum mismatch)".into(),
                ));
            }
            writer.write_all(content)?;
            self.remaining -= plaintext_size as u64;
            self.block_index += 1;
        }

        let mut trailing = [0_u8; 1];
        if self.reader.read(&mut trailing)? != 0 {
            return Err(En3Error::Format(
                "Archive has unexpected trailing bytes".into(),
            ));
        }
        writer.flush()?;
        Ok(DecodeInfo {
            filename: self.filename,
            original_size: self.original_size,
        })
    }
}

/// Decode an EN3 stream incrementally. Neither input nor output is loaded whole.
pub fn decode<R: Read, W: Write>(reader: R, mut writer: W, password: &str) -> Result<DecodeInfo> {
    Decoder::new(reader, password)?.decode_to(&mut writer)
}

fn temporary_output_path(output: &Path, attempt: u32) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut name = OsString::from(".");
    name.push(
        output
            .file_name()
            .unwrap_or_else(|| "recovered.bin".as_ref()),
    );
    name.push(format!(".{}.{nanos}.{attempt}.tmp", std::process::id()));
    output.with_file_name(name)
}

fn atomic_write<T>(output: &Path, operation: impl FnOnce(&mut File) -> Result<T>) -> Result<T> {
    if output.exists() {
        return Err(En3Error::OutputExists(output.to_path_buf()));
    }
    let mut temporary = None;
    for attempt in 0..100 {
        let candidate = temporary_output_path(output, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(En3Error::Io(error)),
        }
    }
    let (temporary_path, mut temporary_file) = temporary
        .ok_or_else(|| En3Error::Format("Could not allocate a temporary output file".into()))?;
    let result = (|| {
        let value = operation(&mut temporary_file)?;
        temporary_file.sync_all()?;
        fs::rename(&temporary_path, output)?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

/// Recover an archive to a new file. Existing outputs are never overwritten.
/// Data is decoded beside the destination and renamed only after full validation.
pub fn recover_file(archive: &Path, password: &str, output: Option<&Path>) -> Result<RecoveryInfo> {
    recover_file_monitored(archive, password, output, |_| Ok(()))
}

pub fn recover_file_monitored<F: FnMut(u64) -> io::Result<()>>(
    archive: &Path,
    password: &str,
    output: Option<&Path>,
    on_progress: F,
) -> Result<RecoveryInfo> {
    let input = MonitoredReader::new(File::open(archive)?, on_progress);
    let decoder = Decoder::new(input, password)?;
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| archive.with_file_name(decoder.filename.clone()));
    let filename = decoder.filename.clone();
    let original_size = decoder.original_size;
    atomic_write(&output, |file| decoder.decode_to(file))?;
    Ok(RecoveryInfo {
        filename,
        original_size,
        output,
    })
}
