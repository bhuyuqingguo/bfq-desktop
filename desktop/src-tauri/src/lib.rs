use en3_recovery::bundle::{decrypt_path_monitored, encrypt_prepared_monitored, prepare_input};
use en3_recovery::modern::{choose_output, load_keyfile, migrate_en3_monitored, verify_file_monitored};
use en3_recovery::recover_file_monitored;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{Emitter, WebviewWindow};

#[derive(Default)]
struct AppState {
    running: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}
struct RunningGuard(Arc<AtomicBool>);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationRequest {
    operation: String,
    input: String,
    password: String,
    new_password: Option<String>,
    output: Option<String>,
    keyfile: Option<String>,
    #[serde(default)]
    compress: bool,
    #[serde(default)]
    rename_existing: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationResult {
    filename: String,
    original_size: u64,
    output: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    processed: u64,
    total: u64,
    percent: u8,
}

fn begin(state: &AppState) -> Result<RunningGuard, String> {
    state
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "已有任务正在执行".to_string())?;
    state.cancelled.store(false, Ordering::Release);
    Ok(RunningGuard(Arc::clone(&state.running)))
}
fn output_path(output: Option<&str>) -> Option<PathBuf> {
    output.filter(|v| !v.is_empty()).map(PathBuf::from)
}
fn map_error(error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    if message.contains("Operation cancelled") {
        "操作已取消，未生成输出文件".into()
    } else if message.contains("requires a key file") {
        "此封装需要使用密钥文件，请选择密钥文件".into()
    } else if message.contains("without a key file") {
        "此封装未使用密钥文件，请留空密钥文件".into()
    } else if message.contains("Key file is empty") {
        "密钥文件为空，无法使用".into()
    } else {
        message
    }
}

#[tauri::command]
async fn execute_operation(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    request: OperationRequest,
) -> Result<OperationResult, String> {
    let input = PathBuf::from(&request.input);
    let guard = begin(&state)?;
    let cancelled = Arc::clone(&state.cancelled);
    let output = output_path(request.output.as_deref());
    let keyfile = request
        .keyfile
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(Path::new)
        .map(load_keyfile)
        .transpose()
        .map_err(map_error)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let digest = keyfile.as_deref();
        let prepared = if request.operation == "pack" {
            Some(prepare_input(&input, request.compress).map_err(map_error)?)
        } else {
            None
        };
        let total = prepared.as_ref().map_or_else(
            || input.metadata().map(|m| m.len()).unwrap_or(0),
            |p| p.stream_size(),
        );
        let mut last_emit = Instant::now() - Duration::from_millis(100);
        let mut last_percent = u8::MAX;
        let progress = move |processed: u64| {
            if cancelled.load(Ordering::Acquire) {
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
                let _ = window.emit(
                    "operation-progress",
                    ProgressPayload {
                        processed,
                        total,
                        percent,
                    },
                );
                last_emit = Instant::now();
                last_percent = percent;
            }
            Ok(())
        };
        let result = match request.operation.as_str() {
            "pack" => encrypt_prepared_monitored(
                prepared.expect("prepared pack input"),
                &input,
                &request.password,
                digest,
                output.as_deref(),
                request.rename_existing,
                progress,
            ),
            "unpack" => decrypt_path_monitored(
                &input,
                &request.password,
                digest,
                output.as_deref(),
                request.rename_existing,
                progress,
            ),
            "verify" => verify_file_monitored(&input, &request.password, digest, progress),
            "recover" => {
                recover_file_monitored(&input, &request.password, output.as_deref(), progress).map(
                    |info| en3_recovery::modern::FileInfo {
                        filename: info.filename,
                        original_size: info.original_size,
                        output: info.output,
                    },
                )
            }
            "migrate" => {
                let new_password = request
                    .new_password
                    .as_deref()
                    .ok_or_else(|| "缺少新口令".to_string())?;
                let requested = output.unwrap_or_else(|| input.with_extension("bfq"));
                let destination =
                    choose_output(&requested, request.rename_existing).map_err(map_error)?;
                migrate_en3_monitored(
                    &input,
                    &request.password,
                    new_password,
                    digest,
                    Some(&destination),
                    progress,
                )
            }
            _ => return Err("不支持的操作".into()),
        }
        .map_err(map_error)?;
        Ok(OperationResult {
            filename: result.filename,
            original_size: result.original_size,
            output: result.output.to_string_lossy().into_owned(),
        })
    })
    .await
    .map_err(|error| format!("任务执行失败：{error}"))?
}

#[tauri::command]
fn cancel_operation(state: tauri::State<'_, AppState>) {
    if state.running.load(Ordering::Acquire) {
        state.cancelled.store(true, Ordering::Release);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            execute_operation,
            cancel_operation
        ])
        .run(tauri::generate_context!())
        .expect("error while running BFQ desktop application");
}
