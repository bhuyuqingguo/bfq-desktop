use en3_recovery::bundle::{decrypt_path_monitored, encrypt_prepared_monitored, prepare_input};
use en3_recovery::modern::{FileInfo, choose_output, migrate_en3_monitored, verify_file_monitored};
use en3_recovery::recover_file_monitored;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    operation: Operation,
    input: PathBuf,
    password: String,
    new_password: Option<String>,
    output: Option<PathBuf>,
    #[serde(default)]
    compress: bool,
    #[serde(default)]
    rename_existing: bool,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Pack,
    Unpack,
    Verify,
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
    prepared: Option<en3_recovery::bundle::PreparedInput>,
    progress: impl FnMut(u64) -> io::Result<()>,
) -> en3_recovery::Result<FileInfo> {
    let output = request.output.as_deref();
    match request.operation {
        Operation::Pack => encrypt_prepared_monitored(
            prepared
                .ok_or_else(|| en3_recovery::En3Error::Format("Missing prepared input".into()))?,
            &request.input,
            &request.password,
            output,
            request.rename_existing,
            progress,
        ),
        Operation::Unpack => decrypt_path_monitored(
            &request.input,
            &request.password,
            output,
            request.rename_existing,
            progress,
        ),
        Operation::Verify => verify_file_monitored(&request.input, &request.password, progress),
        Operation::Migrate => {
            let new_password = request
                .new_password
                .as_deref()
                .ok_or_else(|| en3_recovery::En3Error::Format("缺少新口令".into()))?;
            let requested = request
                .output
                .clone()
                .unwrap_or_else(|| request.input.with_extension("bfq"));
            let destination = choose_output(&requested, request.rename_existing)?;
            migrate_en3_monitored(
                &request.input,
                &request.password,
                new_password,
                Some(&destination),
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
    let mut request_bytes = Vec::new();
    io::stdin().lock().read_to_end(&mut request_bytes)?;
    let request: Request = serde_json::from_slice(&request_bytes)?;
    let prepared = if matches!(request.operation, Operation::Pack) {
        Some(prepare_input(&request.input, request.compress)?)
    } else {
        None
    };
    let total = prepared.as_ref().map_or_else(
        || request.input.metadata().map(|m| m.len()).unwrap_or(0),
        |p| p.stream_size(),
    );
    let mut output = io::stdout().lock();
    write_event(&mut output, json!({"type":"started","total":total}))?;
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
                json!({"type":"progress","processed":processed,"total":total,"percent":percent}),
            )?;
            last_emit = Instant::now();
            last_percent = percent;
        }
        Ok(())
    };
    let result = OperationResult::from(execute(&request, prepared, progress)?);
    write_event(
        &mut output,
        json!({"type":"result","filename":result.filename,"originalSize":result.original_size,"output":result.output}),
    )?;
    Ok(())
}
fn print_usage() {
    println!(
        "观海·八分共享核心\n\n用法：guanhai-core request [--cancel-file <路径>]\n从标准输入读取一个 JSON 操作请求，逐行输出 JSON 进度和结果。\n口令只通过标准输入传递，不写入命令行或日志。"
    );
}
fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }
    let result = if arguments.first().is_some_and(|v| v == "request") {
        parse_cancel_file(&arguments[1..]).and_then(run)
    } else {
        Err("用法：guanhai-core request [--cancel-file <路径>]".into())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = write_event(
                &mut io::stdout().lock(),
                json!({"type":"error","message":friendly_error(&error.to_string())}),
            );
            ExitCode::FAILURE
        }
    }
}
