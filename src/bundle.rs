//! File/folder preparation and safe restoration around the versioned BFQ stream.
//! Ordinary BFQ files remain byte-compatible; ZIP markers are internal to new bundles.

use super::{En3Error, MonitoredReader, Result, atomic_write, safe_basename};
use crate::modern::{
    BFQ_EXTENSION, FileInfo, StreamInfo, choose_output, decrypt, decrypt_file_monitored, encrypt,
    inspect_file,
};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const FILE_BUNDLE: &str = "BFQ_FILE_V1";
const DIRECTORY_BUNDLE: &str = "BFQ_DIRECTORY_V1";

pub struct PreparedInput {
    path: PathBuf,
    filename: String,
    source_size: u64,
    _temporary: Option<NamedTempFile>,
}

impl PreparedInput {
    pub fn stream_size(&self) -> u64 {
        self.path.metadata().map(|value| value.len()).unwrap_or(0)
    }
}

fn unicode_filename(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| En3Error::Format("Input filename must be valid Unicode".into()))
}

fn zip_options(compress: bool) -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(if compress {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    })
}

fn write_file_entry<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    path: &Path,
    name: String,
    options: SimpleFileOptions,
) -> Result<u64> {
    if path.symlink_metadata()?.file_type().is_symlink() {
        return Err(En3Error::Format("Symbolic links are not supported".into()));
    }
    let size = path.metadata()?.len();
    zip.start_file(name, options)?;
    let mut reader = File::open(path)?;
    io::copy(&mut reader, zip)?;
    Ok(size)
}

pub fn prepare_input(input: &Path, compress: bool) -> Result<PreparedInput> {
    let filename = unicode_filename(input)?;
    if input.is_file() && !compress {
        return Ok(PreparedInput {
            path: input.to_path_buf(),
            source_size: input.metadata()?.len(),
            filename,
            _temporary: None,
        });
    }
    if !input.is_file() && !input.is_dir() {
        return Err(En3Error::Format("Input must be a file or folder".into()));
    }
    let directory = input.is_dir();
    let temporary = NamedTempFile::new()?;
    let options = zip_options(compress);
    let mut source_size = 0_u64;
    {
        let output = temporary.reopen()?;
        let mut zip = ZipWriter::new(output);
        zip.set_comment(if directory {
            DIRECTORY_BUNDLE
        } else {
            FILE_BUNDLE
        });
        if directory {
            zip.add_directory(format!("{filename}/"), options)?;
            for entry in WalkDir::new(input)
                .min_depth(1)
                .follow_links(false)
                .sort_by_file_name()
            {
                let entry = entry?;
                let relative = entry.path().strip_prefix(input).map_err(|error| {
                    En3Error::Format(format!("Unable to prepare folder: {error}"))
                })?;
                let relative = relative.to_string_lossy().replace('\\', "/");
                let archived = format!("{filename}/{relative}");
                if entry.file_type().is_dir() {
                    zip.add_directory(format!("{archived}/"), options)?;
                } else if entry.file_type().is_file() {
                    source_size = source_size.saturating_add(write_file_entry(
                        &mut zip,
                        entry.path(),
                        archived,
                        options,
                    )?);
                } else {
                    return Err(En3Error::Format(
                        "Only ordinary files and folders are supported".into(),
                    ));
                }
            }
        } else {
            source_size = write_file_entry(&mut zip, input, filename.clone(), options)?;
        }
        zip.finish()?;
    }
    Ok(PreparedInput {
        path: temporary.path().to_path_buf(),
        filename: format!("{filename}.zip"),
        source_size,
        _temporary: Some(temporary),
    })
}

pub fn encrypt_prepared_monitored<F: FnMut(u64) -> io::Result<()>>(
    prepared: PreparedInput,
    input: &Path,
    password: &str,
    output: Option<&Path>,
    rename_existing: bool,
    on_progress: F,
) -> Result<FileInfo> {
    let requested = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| input.with_extension(BFQ_EXTENSION));
    let output = choose_output(&requested, rename_existing)?;
    let stream_size = prepared.stream_size();
    let reader = MonitoredReader::new(File::open(&prepared.path)?, on_progress);
    atomic_write(&output, |file| {
        encrypt(reader, file, password, &prepared.filename, stream_size)
    })?;
    Ok(FileInfo {
        filename: prepared.filename,
        original_size: prepared.source_size,
        output,
    })
}

pub fn encrypt_path_monitored<F: FnMut(u64) -> io::Result<()>>(
    input: &Path,
    password: &str,
    output: Option<&Path>,
    compress: bool,
    rename_existing: bool,
    on_progress: F,
) -> Result<FileInfo> {
    let prepared = prepare_input(input, compress)?;
    encrypt_prepared_monitored(
        prepared,
        input,
        password,
        output,
        rename_existing,
        on_progress,
    )
}

fn copy_temp_to_output(temp: &mut NamedTempFile, output: &Path) -> Result<()> {
    atomic_write(output, |file| {
        let mut reader = temp.reopen()?;
        io::copy(&mut reader, file)?;
        Ok(())
    })?;
    Ok(())
}

fn checked_entry_path(root: &Path, entry: &zip::read::ZipFile<File>) -> Result<PathBuf> {
    let relative = entry
        .enclosed_name()
        .ok_or_else(|| En3Error::Format("Unsafe path in BFQ folder bundle".into()))?;
    if entry
        .unix_mode()
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err(En3Error::Format(
            "Symbolic links are not supported in BFQ bundles".into(),
        ));
    }
    Ok(root.join(relative))
}

fn restore_directory(mut archive: ZipArchive<File>, output: &Path, root_name: &str) -> Result<()> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".bfq-folder-")
        .tempdir_in(parent)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let target = checked_entry_path(staging.path(), &entry)?;
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = File::create_new(&target)?;
            io::copy(&mut entry, &mut file)?;
        }
    }
    let restored_root = staging.path().join(root_name);
    if !restored_root.is_dir() {
        return Err(En3Error::Format("Invalid BFQ folder root".into()));
    }
    fs::rename(restored_root, output)?;
    Ok(())
}

fn restore_zip_bundle(
    temporary: &mut NamedTempFile,
    archive_path: &Path,
    requested: Option<&Path>,
    rename_existing: bool,
    info: &StreamInfo,
) -> Result<FileInfo> {
    let mut archive = ZipArchive::new(temporary.reopen()?)?;
    let marker = archive.comment().to_vec();
    if marker != FILE_BUNDLE.as_bytes() && marker != DIRECTORY_BUNDLE.as_bytes() {
        let output = choose_output(
            requested.unwrap_or(&archive_path.with_file_name(&info.filename)),
            rename_existing,
        )?;
        copy_temp_to_output(temporary, &output)?;
        return Ok(FileInfo {
            filename: info.filename.clone(),
            original_size: info.original_size,
            output,
        });
    }
    if marker == FILE_BUNDLE.as_bytes() {
        if archive.len() != 1 {
            return Err(En3Error::Format("Invalid BFQ file bundle".into()));
        }
        let mut entry = archive.by_index(0)?;
        let filename = safe_basename(entry.name())?.to_string();
        let output = choose_output(
            requested.unwrap_or(&archive_path.with_file_name(&filename)),
            rename_existing,
        )?;
        atomic_write(&output, |file| {
            io::copy(&mut entry, file)?;
            Ok(())
        })?;
        return Ok(FileInfo {
            filename,
            original_size: info.original_size,
            output,
        });
    }
    let root_name = info
        .filename
        .strip_suffix(".zip")
        .ok_or_else(|| En3Error::Format("Invalid BFQ folder bundle name".into()))?;
    let root_name = safe_basename(root_name)?.to_string();
    let output = choose_output(
        requested.unwrap_or(&archive_path.with_file_name(&root_name)),
        rename_existing,
    )?;
    restore_directory(archive, &output, &root_name)?;
    Ok(FileInfo {
        filename: root_name,
        original_size: info.original_size,
        output,
    })
}

pub fn decrypt_path_monitored<F: FnMut(u64) -> io::Result<()>>(
    archive_path: &Path,
    password: &str,
    output: Option<&Path>,
    rename_existing: bool,
    on_progress: F,
) -> Result<FileInfo> {
    let info = inspect_file(archive_path, password)?;
    if !info.filename.to_ascii_lowercase().ends_with(".zip") {
        let output = choose_output(
            output.unwrap_or(&archive_path.with_file_name(&info.filename)),
            rename_existing,
        )?;
        return decrypt_file_monitored(archive_path, password, Some(&output), on_progress);
    }
    let mut temporary = NamedTempFile::new()?;
    let reader = MonitoredReader::new(File::open(archive_path)?, on_progress);
    decrypt(reader, &mut temporary.as_file_mut(), password)?;
    restore_zip_bundle(&mut temporary, archive_path, output, rename_existing, &info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_roundtrip_and_rename() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("资料");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("说明.txt"), b"hello folder").unwrap();
        fs::create_dir(source.join("空目录")).unwrap();
        let archive = root.path().join("资料.bfq");
        encrypt_path_monitored(&source, "password", Some(&archive), true, false, |_| Ok(()))
            .unwrap();
        fs::remove_dir_all(&source).unwrap();
        let restored =
            decrypt_path_monitored(&archive, "password", None, false, |_| Ok(())).unwrap();
        assert_eq!(restored.output, root.path().join("资料"));
        assert_eq!(
            fs::read(restored.output.join("说明.txt")).unwrap(),
            b"hello folder"
        );
        assert!(restored.output.join("空目录").is_dir());
        let renamed = decrypt_path_monitored(&archive, "password", None, true, |_| Ok(())).unwrap();
        assert_eq!(renamed.output, root.path().join("资料-1"));
    }

    #[test]
    fn compressed_file_restores_original() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("重复.txt");
        fs::write(&source, vec![42; 100_000]).unwrap();
        let archive = root.path().join("压缩.bfq");
        encrypt_path_monitored(&source, "password", Some(&archive), true, false, |_| Ok(()))
            .unwrap();
        assert!(archive.metadata().unwrap().len() < source.metadata().unwrap().len());
        fs::remove_file(&source).unwrap();
        let restored =
            decrypt_path_monitored(&archive, "password", None, false, |_| Ok(())).unwrap();
        assert_eq!(restored.filename, "重复.txt");
        assert_eq!(fs::read(restored.output).unwrap(), vec![42; 100_000]);
    }

    #[test]
    fn ordinary_zip_is_not_expanded() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("普通.zip");
        {
            let mut zip = ZipWriter::new(File::create(&source).unwrap());
            zip.start_file("hello.txt", zip_options(false)).unwrap();
            zip.write_all(b"ordinary").unwrap();
            zip.finish().unwrap();
        }
        let original = fs::read(&source).unwrap();
        let archive = root.path().join("普通.bfq");
        encrypt_path_monitored(
            &source,
            "password",
            Some(&archive),
            false,
            false,
            |_| Ok(()),
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        let restored =
            decrypt_path_monitored(&archive, "password", None, false, |_| Ok(())).unwrap();
        assert_eq!(restored.output, source);
        assert_eq!(fs::read(restored.output).unwrap(), original);
    }
}
