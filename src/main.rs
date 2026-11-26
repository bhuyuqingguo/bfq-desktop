use en3_recovery::modern::{decrypt_file, encrypt_file, load_keyfile, migrate_en3};
use en3_recovery::recover_file;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use zeroize::Zeroizing;

fn usage() {
    println!("EN3 recovery and BFQ authenticated encryption");
    println!();
    println!("Usage:");
    println!("  en3-recovery pack <input> <password> [--keyfile <file>] [--output <archive.bfq>]");
    println!("  en3-recovery unpack <archive.bfq> <password> [--keyfile <file>] [--output <file>]");
    println!(
        "  en3-recovery migrate <archive.en3> <old-password> <new-password> [--keyfile <file>] [--output <archive.bfq>]"
    );
    println!("  en3-recovery recover <archive.en3> <password> [--output <file>]");
    println!();
    println!("Legacy shorthand remains supported:");
    println!("  en3-recovery <archive.en3> <password> [--output <file>]");
    println!();
    println!("BFQ encrypts both content and filename using Argon2id + AES-256-GCM.");
    println!("An optional key file adds a second factor to BFQ operations.");
    println!("Existing output files are never overwritten; passwords are never logged.");
}

fn text(argument: &OsString) -> Result<&str, Box<dyn Error>> {
    argument
        .to_str()
        .ok_or_else(|| "password and command arguments must be valid Unicode".into())
}

struct Options {
    output: Option<PathBuf>,
    keyfile: Option<PathBuf>,
}

fn parse_options(arguments: &[OsString]) -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        output: None,
        keyfile: None,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--output") | Some("-o") if index + 1 < arguments.len() => {
                options.output = Some(PathBuf::from(&arguments[index + 1]));
                index += 2;
            }
            Some("--keyfile") if index + 1 < arguments.len() => {
                options.keyfile = Some(PathBuf::from(&arguments[index + 1]));
                index += 2;
            }
            _ => {
                return Err(format!("unknown or incomplete option: {:?}", arguments[index]).into());
            }
        }
    }
    Ok(options)
}

fn load_digest(path: Option<&Path>) -> Result<Option<Zeroizing<[u8; 32]>>, Box<dyn Error>> {
    path.map(|value| load_keyfile(value).map_err(|error| -> Box<dyn Error> { error.to_string().into() }))
        .transpose()
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
            let options = parse_options(&arguments[3..])?;
            match command {
                "pack" => {
                    let keyfile = load_digest(options.keyfile.as_deref())?;
                    let info = encrypt_file(
                        &input,
                        password,
                        keyfile.as_deref(),
                        options.output.as_deref(),
                    )?;
                    print_info(&info.filename, info.original_size, &info.output);
                }
                "unpack" => {
                    let keyfile = load_digest(options.keyfile.as_deref())?;
                    let info = decrypt_file(
                        &input,
                        password,
                        keyfile.as_deref(),
                        options.output.as_deref(),
                    )?;
                    print_info(&info.filename, info.original_size, &info.output);
                }
                _ => {
                    let info = recover_file(&input, password, options.output.as_deref())?;
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
            let options = parse_options(&arguments[4..])?;
            let keyfile = load_digest(options.keyfile.as_deref())?;
            let info = migrate_en3(
                &input,
                old_password,
                new_password,
                keyfile.as_deref(),
                options.output.as_deref(),
            )?;
            print_info(&info.filename, info.original_size, &info.output);
        }
        _ => {
            if arguments.len() < 2 {
                usage();
                return Err("missing archive or password".into());
            }
            let input = PathBuf::from(&arguments[0]);
            let password = text(&arguments[1])?;
            let options = parse_options(&arguments[2..])?;
            let info = recover_file(&input, password, options.output.as_deref())?;
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
