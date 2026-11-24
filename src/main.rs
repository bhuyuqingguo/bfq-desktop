use en3_recovery::modern::{decrypt_file, encrypt_file, migrate_en3};
use en3_recovery::recover_file;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() {
    println!("EN3 recovery and BFQ authenticated encryption");
    println!();
    println!("Usage:");
    println!("  en3-recovery pack <input> <password> [--output <archive.bfq>]");
    println!("  en3-recovery unpack <archive.bfq> <password> [--output <file>]");
    println!(
        "  en3-recovery migrate <archive.en3> <old-password> <new-password> [--output <archive.bfq>]"
    );
    println!("  en3-recovery recover <archive.en3> <password> [--output <file>]");
    println!();
    println!("Legacy shorthand remains supported:");
    println!("  en3-recovery <archive.en3> <password> [--output <file>]");
    println!();
    println!("BFQ encrypts both content and filename using Argon2id + AES-256-GCM.");
    println!("Existing output files are never overwritten; passwords are never logged.");
}

fn text(argument: &OsString) -> Result<&str, Box<dyn Error>> {
    argument
        .to_str()
        .ok_or_else(|| "password and command arguments must be valid Unicode".into())
}

fn output_argument(arguments: &[OsString]) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--output") | Some("-o") if index + 1 < arguments.len() => {
                output = Some(PathBuf::from(&arguments[index + 1]));
                index += 2;
            }
            _ => {
                return Err(format!("unknown or incomplete option: {:?}", arguments[index]).into());
            }
        }
    }
    Ok(output)
}

fn print_info(filename: &str, size: u64, output: &Path) {
    println!("filename={filename}");
    println!("size={size}");
    println!("output={}", output.display());
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        usage();
        return Ok(());
    }
    if arguments.is_empty() {
        usage();
        return Err("missing command".into());
    }

    let command = arguments[0].to_str().unwrap_or("recover");
    match command {
        "pack" | "unpack" | "recover" => {
            if arguments.len() < 3 {
                return Err(format!("{command} requires an input path and password").into());
            }
            let input = PathBuf::from(&arguments[1]);
            let password = text(&arguments[2])?;
            let output = output_argument(&arguments[3..])?;
            match command {
                "pack" => {
                    let info = encrypt_file(&input, password, output.as_deref())?;
                    print_info(&info.filename, info.original_size, &info.output);
                }
                "unpack" => {
                    let info = decrypt_file(&input, password, output.as_deref())?;
                    print_info(&info.filename, info.original_size, &info.output);
                }
                _ => {
                    let info = recover_file(&input, password, output.as_deref())?;
                    print_info(&info.filename, info.original_size, &info.output);
                }
            }
        }
        "migrate" => {
            if arguments.len() < 4 {
                return Err("migrate requires an EN3 path, old password, and new password".into());
            }
            let input = PathBuf::from(&arguments[1]);
            let old_password = text(&arguments[2])?;
            let new_password = text(&arguments[3])?;
            let output = output_argument(&arguments[4..])?;
            let info = migrate_en3(&input, old_password, new_password, output.as_deref())?;
            print_info(&info.filename, info.original_size, &info.output);
        }
        _ => {
            if arguments.len() < 2 {
                usage();
                return Err("missing archive or password".into());
            }
            let input = PathBuf::from(&arguments[0]);
            let password = text(&arguments[1])?;
            let output = output_argument(&arguments[2..])?;
            let info = recover_file(&input, password, output.as_deref())?;
            print_info(&info.filename, info.original_size, &info.output);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
