use en3_recovery::modern::{
    FileInfo, decrypt_file_monitored, encrypt_file_monitored, migrate_en3_monitored,
};
use en3_recovery::recover_file_monitored;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    operation: Operation,
    input: PathBuf,
    password: String,
    new_password: Option<String>,
    output: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Pack,
    Unpack,
    Migrate,
    Recover,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationResult {
    filename: String,
    original_size: u64,
    output: String,
}

impl From<FileInfo> for OperationResult {
    fn from(info: FileInfo) -> Self {
        Self {
            filename: info.filename,
            original_size: info.original_size,
            output: info.output.to_string_lossy().into_owned(),
        }
    }
}

fn write_event(output: &mut impl Write, event: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &event)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn execute(
    request: &Request,
    progress: impl FnMut(u64) -> io::Result<()>,
) -> en3_recovery::Result<FileInfo> {
    let output = request.output.as_deref();
    match request.operation {
        Operation::Pack => {
            encrypt_file_monitored(&request.input, &request.password, output, progress)
        }
        Operation::Unpack => {
            decrypt_file_monitored(&request.input, &request.password, output, progress)
        }
        Operation::Migrate => {
            let new_password = request
                .new_password
                .as_deref()
                .ok_or_else(|| en3_recovery::En3Error::Format("缺少新口令".into()))?;
            migrate_en3_monitored(
                &request.input,
                &request.password,
                new_password,
                output,
                progress,
            )
        }
        Operation::Recover => {
            recover_file_monitored(&request.input, &request.password, output, progress).map(
                |info| FileInfo {
                    filename: info.filename,
                    original_size: info.original_size,
                    output: info.output,
                },
            )
        }
    }
}

fn friendly_error(message: &str) -> String {
    if message.contains("Operation cancelled") {
        "操作已取消，未生成输出文件".into()
    } else if message.contains("Output already exists") {
        "目标文件已存在，未覆盖任何文件".into()
    } else if message.contains("Wrong password") || message.contains("authentication failed") {
        "口令不正确，或文件已经损坏".into()
    } else if message.contains("Not an enc3") || message.contains("Not a BFQ") {
        "所选文件与当前操作模式不匹配".into()
    } else {
        message.to_string()
    }
}

fn parse_cancel_file(arguments: &[OsString]) -> Result<Option<PathBuf>, Box<dyn Error>> {
    match arguments {
        [] => Ok(None),
        [option, path] if option == "--cancel-file" => Ok(Some(PathBuf::from(path))),
        _ => Err("用法：guanhai-core request [--cancel-file <路径>]".into()),
    }
}

fn run(cancel_file: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut input = io::stdin().lock();
    let mut request_bytes = Vec::new();
    input.read_to_end(&mut request_bytes)?;
    let request: Request = serde_json::from_slice(&request_bytes)?;
    let total = request.input.metadata()?.len();
    let mut output = io::stdout().lock();
    write_event(&mut output, json!({ "type": "started", "total": total }))?;

    let mut last_emit = Instant::now() - Duration::from_millis(100);
    let mut last_percent = u8::MAX;
    let progress = |processed: u64| {
        if cancel_file.as_ref().is_some_and(|path| path.exists()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Operation cancelled",
            ));
        }
        let percent = processed
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(100)
            .min(100) as u8;
        if percent != last_percent
            && (processed == total || last_emit.elapsed() >= Duration::from_millis(60))
        {
            write_event(
                &mut output,
                json!({
                    "type": "progress",
                    "processed": processed,
                    "total": total,
                    "percent": percent,
                }),
            )?;
            last_emit = Instant::now();
            last_percent = percent;
        }
        Ok(())
    };

    let result = OperationResult::from(execute(&request, progress)?);
    write_event(
        &mut output,
        json!({
            "type": "result",
            "filename": result.filename,
            "originalSize": result.original_size,
            "output": result.output,
        }),
    )?;
    Ok(())
}

fn print_usage() {
    println!("观海·八分共享核心");
    println!();
    println!("用法：guanhai-core request [--cancel-file <路径>]");
    println!("从标准输入读取一个 JSON 操作请求，逐行输出 JSON 进度和结果。");
    println!("口令只通过标准输入传递，不写入命令行或日志。");
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }
    let result = if arguments.first().is_some_and(|value| value == "request") {
        parse_cancel_file(&arguments[1..]).and_then(run)
    } else {
        Err("用法：guanhai-core request [--cancel-file <路径>]".into())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = friendly_error(&error.to_string());
            let _ = write_event(
                &mut io::stdout().lock(),
                json!({ "type": "error", "message": message }),
            );
            ExitCode::FAILURE
        }
    }
}
