#![windows_subsystem = "windows"]

#[cfg(windows)]
mod win7ui {

    use en3_recovery::bundle::{decrypt_path_monitored, encrypt_prepared_monitored, prepare_input};
    use en3_recovery::modern::{choose_output, verify_file_monitored};
    use en3_recovery::recover_file_monitored;
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard};
    use winapi::shared::basetsd::LONG_PTR;
    use winapi::shared::minwindef::*;
    use winapi::shared::windef::{HBRUSH, HDC, HGDIOBJ, HMENU, HWND};
    use winapi::um::commctrl::{PBM_SETPOS, PBM_SETRANGE32};
    use winapi::um::commdlg::*;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::shellapi::HDROP;
    use winapi::um::shellapi::*;
    use winapi::um::wingdi::{CreateSolidBrush, DeleteObject, RGB, SetBkMode, TRANSPARENT};
    use winapi::um::winuser::*;

    const CLASS_NAME: &str = "GuanhaiBafenWin7";
    const WINDOW_TITLE: &str = "观海·八分 0.0.1｜Windows 7 兼容版";
    const TIMER_ID: usize = 1;
    const MAX_PATH_LEN: usize = 260;

    const IDC_INPUT: i32 = 101;
    const IDC_INPUT_FILE: i32 = 102;
    const IDC_OUTPUT: i32 = 104;
    const IDC_OUTPUT_BROWSE: i32 = 105;
    const IDC_PASSWORD: i32 = 106;
    const IDC_CONFIRM: i32 = 107;
    const IDC_COMPRESS: i32 = 108;
    const IDC_RENAME: i32 = 109;
    const IDC_PROGRESS: i32 = 110;
    const IDC_STATUS: i32 = 111;
    const IDC_START: i32 = 112;
    const IDC_CANCEL: i32 = 113;
    const IDC_OP_PACK: i32 = 114;
    const IDC_OP_UNPACK: i32 = 115;
    const IDC_OP_VERIFY: i32 = 116;
    const IDC_OP_RECOVER: i32 = 117;
    const BST_CHECKED_U: UINT = BST_CHECKED as UINT;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Operation {
        Pack,
        Unpack,
        Verify,
        Recover,
    }

    struct SharedState {
        progress: u8,
        running: bool,
        result: Option<std::result::Result<String, String>>,
    }

    struct App {
        input: HWND,
        output: HWND,
        password: HWND,
        confirm: HWND,
        compress: HWND,
        rename: HWND,
        progress: HWND,
        status: HWND,
        start: HWND,
        cancel: HWND,
        shared: Arc<Mutex<SharedState>>,
        cancel_requested: Arc<AtomicBool>,
        background: HBRUSH,
    }

    impl App {
        fn lock(&self) -> MutexGuard<'_, SharedState> {
            self.shared
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        }

        fn set_running(&self, running: bool) {
            unsafe {
                EnableWindow(self.start, if running { FALSE } else { TRUE });
                EnableWindow(self.cancel, if running { TRUE } else { FALSE });
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn from_wide(value: &[u16]) -> String {
        let end = value
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(value.len());
        OsString::from_wide(&value[..end])
            .to_string_lossy()
            .into_owned()
    }

    fn get_text(window: HWND) -> String {
        let length = unsafe { GetWindowTextLengthW(window) } as usize;
        let mut buffer = vec![0_u16; length + 1];
        let copied = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
        buffer.truncate(copied as usize);
        OsString::from_wide(&buffer).to_string_lossy().into_owned()
    }

    fn set_text(window: HWND, text: &str) {
        let text = wide(text);
        unsafe {
            SetWindowTextW(window, text.as_ptr());
        }
    }

    fn create_window(
        class: &str,
        title: &str,
        style: DWORD,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        id: i32,
    ) -> HWND {
        let class = wide(class);
        let title = wide(title);
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
                x,
                y,
                width,
                height,
                parent,
                id as HMENU,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    }

    fn create_controls(window: HWND, background: HBRUSH) -> Option<App> {
        let title = create_window(
            "STATIC",
            "观海·八分｜文件封装、恢复与校验",
            WS_CHILD | WS_VISIBLE,
            20,
            14,
            620,
            24,
            window,
            0,
        );
        let subtitle = create_window(
            "STATIC",
            "0.0.1｜Windows 7 SP1 64 位兼容版｜本地运行｜默认不覆盖现有文件",
            WS_CHILD | WS_VISIBLE,
            20,
            40,
            620,
            20,
            window,
            0,
        );
        create_window(
            "BUTTON",
            "操作",
            WS_CHILD | WS_VISIBLE | BS_GROUPBOX,
            20,
            68,
            620,
            58,
            window,
            0,
        );
        let op_pack = create_window(
            "BUTTON",
            "封装文件/文件夹",
            WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON | WS_GROUP | WS_TABSTOP,
            42,
            94,
            138,
            22,
            window,
            IDC_OP_PACK,
        );
        let op_unpack = create_window(
            "BUTTON",
            "恢复 BFQ",
            WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON | WS_TABSTOP,
            198,
            94,
            104,
            22,
            window,
            IDC_OP_UNPACK,
        );
        let op_verify = create_window(
            "BUTTON",
            "校验 BFQ",
            WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON | WS_TABSTOP,
            322,
            94,
            104,
            22,
            window,
            IDC_OP_VERIFY,
        );
        let op_recover = create_window(
            "BUTTON",
            "恢复旧 EN3",
            WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON | WS_TABSTOP,
            446,
            94,
            124,
            22,
            window,
            IDC_OP_RECOVER,
        );
        create_window(
            "STATIC",
            "输入",
            WS_CHILD | WS_VISIBLE,
            20,
            134,
            560,
            18,
            window,
            0,
        );
        let input = create_window(
            "EDIT",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL,
            20,
            155,
            560,
            26,
            window,
            IDC_INPUT,
        );
        let input_file = create_window(
            "BUTTON",
            "选文件",
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON | WS_TABSTOP,
            590,
            153,
            70,
            27,
            window,
            IDC_INPUT_FILE,
        );
        create_window(
            "STATIC",
            "输出（可留空自动命名；文件夹恢复请填写目标文件夹名称）",
            WS_CHILD | WS_VISIBLE,
            20,
            218,
            620,
            18,
            window,
            0,
        );
        let output = create_window(
            "EDIT",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL,
            20,
            239,
            560,
            26,
            window,
            IDC_OUTPUT,
        );
        let output_browse = create_window(
            "BUTTON",
            "浏览",
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON | WS_TABSTOP,
            590,
            237,
            70,
            27,
            window,
            IDC_OUTPUT_BROWSE,
        );
        create_window(
            "STATIC",
            "口令",
            WS_CHILD | WS_VISIBLE,
            20,
            276,
            120,
            18,
            window,
            0,
        );
        let password = create_window(
            "EDIT",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_PASSWORD | ES_AUTOHSCROLL,
            20,
            297,
            300,
            26,
            window,
            IDC_PASSWORD,
        );
        create_window(
            "STATIC",
            "确认口令（仅封装）",
            WS_CHILD | WS_VISIBLE,
            350,
            276,
            220,
            18,
            window,
            0,
        );
        let confirm = create_window(
            "EDIT",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_PASSWORD | ES_AUTOHSCROLL,
            350,
            297,
            300,
            26,
            window,
            IDC_CONFIRM,
        );
        let compress = create_window(
            "BUTTON",
            "封装时压缩",
            WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX | WS_TABSTOP,
            20,
            337,
            130,
            22,
            window,
            IDC_COMPRESS,
        );
        let rename = create_window(
            "BUTTON",
            "重名时自动改名",
            WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX | WS_TABSTOP,
            170,
            337,
            170,
            22,
            window,
            IDC_RENAME,
        );
        let progress = create_window(
            "msctls_progress32",
            "",
            WS_CHILD | WS_VISIBLE,
            20,
            371,
            620,
            20,
            window,
            IDC_PROGRESS,
        );
        let status = create_window(
            "EDIT",
            "请选择输入，输入口令后开始。",
            WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_READONLY | WS_VSCROLL,
            20,
            403,
            620,
            62,
            window,
            IDC_STATUS,
        );
        let start = create_window(
            "BUTTON",
            "开始",
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON | WS_TABSTOP,
            468,
            478,
            78,
            30,
            window,
            IDC_START,
        );
        let cancel = create_window(
            "BUTTON",
            "取消",
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON | WS_TABSTOP,
            562,
            478,
            78,
            30,
            window,
            IDC_CANCEL,
        );

        let controls = [
            title,
            subtitle,
            op_pack,
            op_unpack,
            op_verify,
            op_recover,
            input,
            input_file,
            output,
            output_browse,
            password,
            confirm,
            compress,
            rename,
            progress,
            status,
            start,
            cancel,
        ];
        if controls.iter().any(|control| control.is_null()) {
            return None;
        }
        unsafe {
            DragAcceptFiles(window, TRUE);
            CheckDlgButton(window, IDC_OP_PACK, BST_CHECKED_U);
            CheckDlgButton(window, IDC_RENAME, BST_CHECKED_U);
            SendMessageW(progress, PBM_SETRANGE32, 0, 100);
            SendMessageW(progress, PBM_SETPOS, 0, 0);
            EnableWindow(cancel, FALSE);
        }
        Some(App {
            input,
            output,
            password,
            confirm,
            compress,
            rename,
            progress,
            status,
            start,
            cancel,
            shared: Arc::new(Mutex::new(SharedState {
                progress: 0,
                running: false,
                result: None,
            })),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            background,
        })
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: UINT,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                let background = CreateSolidBrush(RGB(246, 248, 251));
                let app = match create_controls(window, background) {
                    Some(app) => Box::new(app),
                    None => return -1,
                };
                SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(app) as LONG_PTR);
                return 0;
            }
            WM_COMMAND => {
                let app = &*(GetWindowLongPtrW(window, GWLP_USERDATA) as *const App);
                let id = LOWORD(wparam as DWORD) as i32;
                let notification = HIWORD(wparam as DWORD) as u16;
                if notification == BN_CLICKED {
                    match id {
                        IDC_INPUT_FILE => pick_input_file(app),
                        IDC_OUTPUT_BROWSE => pick_output_file(app),
                        IDC_OP_PACK | IDC_OP_UNPACK | IDC_OP_VERIFY | IDC_OP_RECOVER => {
                            CheckDlgButton(
                                window,
                                IDC_OP_PACK,
                                if id == IDC_OP_PACK { BST_CHECKED_U } else { 0 },
                            );
                            CheckDlgButton(
                                window,
                                IDC_OP_UNPACK,
                                if id == IDC_OP_UNPACK {
                                    BST_CHECKED_U
                                } else {
                                    0
                                },
                            );
                            CheckDlgButton(
                                window,
                                IDC_OP_VERIFY,
                                if id == IDC_OP_VERIFY {
                                    BST_CHECKED_U
                                } else {
                                    0
                                },
                            );
                            CheckDlgButton(
                                window,
                                IDC_OP_RECOVER,
                                if id == IDC_OP_RECOVER {
                                    BST_CHECKED_U
                                } else {
                                    0
                                },
                            );
                        }
                        IDC_START => start_operation(window, app),
                        IDC_CANCEL => {
                            app.cancel_requested.store(true, Ordering::Release);
                            set_text(app.status, "正在取消…");
                        }
                        _ => {}
                    }
                }
            }
            WM_TIMER => {
                let app = &mut *(GetWindowLongPtrW(window, GWLP_USERDATA) as *mut App);
                let mut state = app.lock();
                SendMessageW(app.progress, PBM_SETPOS, state.progress as WPARAM, 0);
                if let Some(result) = state.result.take() {
                    drop(state);
                    KillTimer(window, TIMER_ID);
                    app.set_running(false);
                    app.cancel_requested.store(false, Ordering::Release);
                    match result {
                        Ok(message) => {
                            set_text(app.status, &message);
                            SendMessageW(app.progress, PBM_SETPOS, 100, 0);
                        }
                        Err(message) => {
                            set_text(app.status, &message);
                            MessageBoxW(
                                window,
                                wide(&message).as_ptr(),
                                wide("观海·八分").as_ptr(),
                                MB_OK | MB_ICONERROR,
                            );
                        }
                    }
                }
                return 0;
            }
            WM_DROPFILES => {
                let app = &*(GetWindowLongPtrW(window, GWLP_USERDATA) as *const App);
                let drop = wparam as HDROP;
                let mut buffer = vec![0_u16; MAX_PATH_LEN];
                if DragQueryFileW(drop, 0, buffer.as_mut_ptr(), buffer.len() as UINT) > 0 {
                    set_text(app.input, &from_wide(&buffer));
                }
                DragFinish(drop);
                return 0;
            }
            WM_CTLCOLORSTATIC => {
                let app = &*(GetWindowLongPtrW(window, GWLP_USERDATA) as *const App);
                SetBkMode(wparam as HDC, TRANSPARENT as i32);
                return app.background as LRESULT;
            }
            WM_CLOSE => {
                DestroyWindow(window);
                return 0;
            }
            WM_DESTROY => {
                let app = Box::from_raw(GetWindowLongPtrW(window, GWLP_USERDATA) as *mut App);
                DeleteObject(app.background as HGDIOBJ);
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    fn selected_operation(window: HWND) -> Operation {
        unsafe {
            if IsDlgButtonChecked(window, IDC_OP_UNPACK) == BST_CHECKED_U {
                Operation::Unpack
            } else if IsDlgButtonChecked(window, IDC_OP_VERIFY) == BST_CHECKED_U {
                Operation::Verify
            } else if IsDlgButtonChecked(window, IDC_OP_RECOVER) == BST_CHECKED_U {
                Operation::Recover
            } else {
                Operation::Pack
            }
        }
    }

    fn pick_input_file(app: &App) {
        let filter = wide("可处理文件 (*.bfq;*.en3;*.*)\0*.bfq;*.en3;*\0所有文件 (*.*)\0*.*\0\0");
        if let Some(path) = open_dialog(ptr::null_mut(), "选择输入文件", &filter, true) {
            set_text(app.input, &path.to_string_lossy());
        }
    }

    fn pick_output_file(app: &App) {
        let filter = wide("BFQ 文件 (*.bfq)\0*.bfq\0所有文件 (*.*)\0*.*\0\0");
        if let Some(path) = save_dialog(ptr::null_mut(), "选择输出位置", &filter) {
            set_text(app.output, &path.to_string_lossy());
        }
    }

    fn open_dialog(
        owner: HWND,
        title: &str,
        filter: &[u16],
        file_must_exist: bool,
    ) -> Option<PathBuf> {
        let title = wide(title);
        let mut buffer = vec![0_u16; MAX_PATH_LEN];
        let mut options = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as DWORD,
            hwndOwner: owner,
            hInstance: ptr::null_mut(),
            lpstrFilter: filter.as_ptr(),
            lpstrCustomFilter: ptr::null_mut(),
            nMaxCustFilter: 0,
            nFilterIndex: 1,
            lpstrFile: buffer.as_mut_ptr(),
            nMaxFile: buffer.len() as DWORD,
            lpstrFileTitle: ptr::null_mut(),
            nMaxFileTitle: 0,
            lpstrInitialDir: ptr::null(),
            lpstrTitle: title.as_ptr(),
            Flags: OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
            nFileOffset: 0,
            nFileExtension: 0,
            lpstrDefExt: ptr::null(),
            lCustData: 0,
            lpfnHook: None,
            lpTemplateName: ptr::null(),
            pvReserved: ptr::null_mut(),
            dwReserved: 0,
            FlagsEx: 0,
        };
        if file_must_exist {
            options.Flags |= OFN_FILEMUSTEXIST;
        }
        let ok = unsafe { GetOpenFileNameW(&mut options) };
        if ok == TRUE {
            Some(PathBuf::from(from_wide(&buffer)))
        } else {
            None
        }
    }

    fn save_dialog(owner: HWND, title: &str, filter: &[u16]) -> Option<PathBuf> {
        let title = wide(title);
        let mut buffer = vec![0_u16; MAX_PATH_LEN];
        let mut options = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as DWORD,
            hwndOwner: owner,
            hInstance: ptr::null_mut(),
            lpstrFilter: filter.as_ptr(),
            lpstrCustomFilter: ptr::null_mut(),
            nMaxCustFilter: 0,
            nFilterIndex: 1,
            lpstrFile: buffer.as_mut_ptr(),
            nMaxFile: buffer.len() as DWORD,
            lpstrFileTitle: ptr::null_mut(),
            nMaxFileTitle: 0,
            lpstrInitialDir: ptr::null(),
            lpstrTitle: title.as_ptr(),
            Flags: OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_OVERWRITEPROMPT,
            nFileOffset: 0,
            nFileExtension: 0,
            lpstrDefExt: ptr::null(),
            lCustData: 0,
            lpfnHook: None,
            lpTemplateName: ptr::null(),
            pvReserved: ptr::null_mut(),
            dwReserved: 0,
            FlagsEx: 0,
        };
        let ok = unsafe { GetSaveFileNameW(&mut options) };
        if ok == TRUE {
            Some(PathBuf::from(from_wide(&buffer)))
        } else {
            None
        }
    }

    fn error_text(error: impl std::fmt::Display) -> String {
        let message = error.to_string();
        if message.contains("Operation cancelled") || message.contains("cancelled") {
            "操作已取消，未生成输出文件。".to_string()
        } else if message.contains("Output already exists") {
            "输出文件已存在。为避免覆盖，本次操作已停止。".to_string()
        } else if message.contains("Wrong password") || message.contains("authentication failed") {
            "口令不正确，或文件已损坏。".to_string()
        } else if message.contains("Not an enc3 archive") {
            "这不是可识别的旧版 EN3 文件。".to_string()
        } else if message.contains("Input must be a file or folder") {
            "输入路径不存在，请重新选择文件或文件夹。".to_string()
        } else {
            format!("操作失败：{message}")
        }
    }

    fn run_operation(
        operation: Operation,
        input: PathBuf,
        output: Option<PathBuf>,
        password: String,
        compress: bool,
        rename_existing: bool,
        shared: Arc<Mutex<SharedState>>,
        cancel_requested: Arc<AtomicBool>,
    ) -> std::result::Result<String, String> {
        let total = input.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let make_progress = move |processed: u64| -> io::Result<()> {
            if cancel_requested.load(Ordering::Acquire) {
                return Err(io::Error::new(io::ErrorKind::Other, "cancelled"));
            }
            let percent = if total == 0 {
                100
            } else {
                ((processed.saturating_mul(100)) / total).min(100) as u8
            };
            let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
            state.progress = percent;
            Ok(())
        };

        match operation {
            Operation::Pack => {
                let prepared = prepare_input(&input, compress).map_err(error_text)?;
                let info = encrypt_prepared_monitored(
                    prepared,
                    &input,
                    &password,
                    output.as_deref(),
                    rename_existing,
                    make_progress,
                )
                .map_err(error_text)?;
                Ok(format!(
                    "封装完成：{}（原始大小 {} 字节）",
                    info.output.display(),
                    info.original_size
                ))
            }
            Operation::Unpack => {
                let info = decrypt_path_monitored(
                    &input,
                    &password,
                    output.as_deref(),
                    rename_existing,
                    make_progress,
                )
                .map_err(error_text)?;
                Ok(format!(
                    "恢复完成：{}（原始大小 {} 字节）",
                    info.output.display(),
                    info.original_size
                ))
            }
            Operation::Verify => {
                let info =
                    verify_file_monitored(&input, &password, make_progress).map_err(error_text)?;
                Ok(format!(
                    "校验通过：{}（{} 字节）",
                    info.filename, info.original_size
                ))
            }
            Operation::Recover => {
                let requested = match output {
                    Some(path) => Some(choose_output(&path, rename_existing).map_err(error_text)?),
                    None => None,
                };
                let info =
                    recover_file_monitored(&input, &password, requested.as_deref(), make_progress)
                        .map_err(error_text)?;
                Ok(format!(
                    "旧版文件恢复完成：{}（原始大小 {} 字节）",
                    info.output.display(),
                    info.original_size
                ))
            }
        }
    }

    fn start_operation(window: HWND, app: &App) {
        let operation = selected_operation(window);
        let input_text = get_text(app.input);
        let input = PathBuf::from(&input_text);
        let output_text = get_text(app.output);
        let output = if output_text.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(output_text))
        };
        let password = get_text(app.password);
        let confirm = get_text(app.confirm);
        if !input.is_file() && !input.is_dir() {
            unsafe {
                MessageBoxW(
                    window,
                    wide("请选择存在的文件或文件夹。").as_ptr(),
                    wide("观海·八分").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
        }
        if password.is_empty() {
            unsafe {
                MessageBoxW(
                    window,
                    wide("请输入口令。").as_ptr(),
                    wide("观海·八分").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
        }
        if operation == Operation::Pack && password != confirm {
            unsafe {
                MessageBoxW(
                    window,
                    wide("两次输入的口令不一致。").as_ptr(),
                    wide("观海·八分").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
        }

        let compress = unsafe { IsDlgButtonChecked(window, IDC_COMPRESS) } == BST_CHECKED_U;
        let rename_existing = unsafe { IsDlgButtonChecked(window, IDC_RENAME) } == BST_CHECKED_U;
        let shared = Arc::clone(&app.shared);
        let cancel_requested = Arc::clone(&app.cancel_requested);
        {
            let mut state = app.lock();
            state.progress = 0;
            state.running = true;
            state.result = None;
        }
        app.cancel_requested.store(false, Ordering::Release);
        app.set_running(true);
        set_text(app.status, "正在处理，请稍候…");
        set_text(app.password, "");
        set_text(app.confirm, "");
        unsafe {
            SendMessageW(app.progress, PBM_SETPOS, 0, 0);
            SetTimer(window, TIMER_ID, 80, None);
        }

        std::thread::spawn(move || {
            let result = run_operation(
                operation,
                input,
                output,
                password,
                compress,
                rename_existing,
                Arc::clone(&shared),
                Arc::clone(&cancel_requested),
            );
            let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
            state.running = false;
            state.progress = 100;
            state.result = Some(result);
        });
    }

    fn main() {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as UINT,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
            };
            if RegisterClassExW(&class) == 0 {
                return;
            }
            let title = wide(WINDOW_TITLE);
            let window = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW & !WS_THICKFRAME & !WS_MAXIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                680,
                565,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null_mut(),
            );
            if window.is_null() {
                return;
            }
            ShowWindow(window, SW_SHOW);
            UpdateWindow(window);
            let mut message = std::mem::zeroed::<MSG>();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&mut message);
                DispatchMessageW(&mut message);
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    win7ui::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("guanhai-win7 is only supported on Windows");
    std::process::exit(1);
}
