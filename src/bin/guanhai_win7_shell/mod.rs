use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use winapi::Interface;
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{FALSE, TRUE};
use winapi::shared::windef::HWND;
use winapi::shared::winerror::ERROR_ALREADY_EXISTS;
use winapi::shared::wtypes::{PROPERTYKEY, VT_BOOL, VT_LPWSTR};
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::um::combaseapi::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::handleapi::CloseHandle;
use winapi::um::knownfolders::{FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Programs};
use winapi::um::objbase::COINIT_APARTMENTTHREADED;
use winapi::um::objidl::IPersistFile;
use winapi::um::propidl::PROPVARIANT;
use winapi::um::propkey::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_PreventPinning, PKEY_AppUserModel_RelaunchCommand,
    PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource,
};
use winapi::um::propsys::IPropertyStore;
use winapi::um::shellapi::SHGetPropertyStoreForWindow;
use winapi::um::shlobj::SHGetKnownFolderPath;
use winapi::um::shobjidl_core::IShellLinkW;
use winapi::um::synchapi::CreateMutexW;
use winapi::um::unknwnbase::IUnknown;
use winapi::um::winnt::{HANDLE, HRESULT};
use winapi::um::winuser::{
    FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const APP_ID: &str = "com.guanhai.bafen";
const PRODUCT_NAME: &str = "观海·八分";
const EXE_NAME: &str = "guanhai-bafen-win7-x64.exe";
const SHELL_LINK: GUID = GUID {
    Data1: 0x00021401,
    Data2: 0,
    Data3: 0,
    Data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};

#[link(name = "shell32")]
unsafe extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> HRESULT;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn shell_result(code: HRESULT) -> io::Result<()> {
    if code < 0 {
        return Err(io::Error::other(format!("Windows Shell HRESULT 0x{:08X}", code as u32)));
    }
    Ok(())
}

struct ComPtr<T>(NonNull<T>);

impl<T: Interface> ComPtr<T> {
    fn acquire(call: impl FnOnce(*mut *mut winapi::ctypes::c_void) -> HRESULT) -> io::Result<Self> {
        let mut result = ptr::null_mut();
        shell_result(call(&mut result))?;
        NonNull::new(result.cast()).map(Self).ok_or_else(|| io::Error::other("Windows Shell returned a null interface"))
    }

    fn get(&self) -> &T {
        unsafe { self.0.as_ref() }
    }

    fn query<U: Interface>(&self) -> io::Result<ComPtr<U>> {
        ComPtr::acquire(|result| unsafe {
            (*self.0.as_ptr().cast::<IUnknown>()).QueryInterface(&U::uuidof(), result)
        })
    }
}

impl<T> Drop for ComPtr<T> {
    fn drop(&mut self) {
        unsafe { (*self.0.as_ptr().cast::<IUnknown>()).Release() };
    }
}

pub(super) struct ComApartment;

impl ComApartment {
    pub(super) fn initialize() -> io::Result<Self> {
        shell_result(unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) })?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) fn set_process_identity() -> io::Result<()> {
    shell_result(unsafe { SetCurrentProcessExplicitAppUserModelID(wide(APP_ID).as_ptr()) })
}

pub(super) struct SingleInstance(HANDLE);

impl SingleInstance {
    pub(super) fn acquire(class_name: &[u16]) -> io::Result<Option<Self>> {
        let handle = unsafe { CreateMutexW(ptr::null_mut(), FALSE, wide("Local\\com.guanhai.bafen.win7").as_ptr()) };
        let last_error = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(last_error as i32));
        }
        let instance = Self(handle);
        if last_error != ERROR_ALREADY_EXISTS {
            return Ok(Some(instance));
        }
        for _ in 0..100 {
            let window = unsafe { FindWindowW(class_name.as_ptr(), ptr::null()) };
            if !window.is_null() {
                unsafe {
                    if IsIconic(window) != FALSE {
                        ShowWindow(window, SW_RESTORE);
                    }
                    SetForegroundWindow(window);
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(None)
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn set_text(store: &IPropertyStore, key: &PROPERTYKEY, text: &mut [u16]) -> io::Result<()> {
    // SetValue copies the string. The caller retains ownership of its buffer.
    let mut value: PROPVARIANT = unsafe { std::mem::zeroed() };
    value.vt = VT_LPWSTR as u16;
    unsafe {
        *value.data.pwszVal_mut() = text.as_mut_ptr();
        shell_result(store.SetValue(key, &value))
    }
}

fn window_store(window: HWND) -> io::Result<ComPtr<IPropertyStore>> {
    ComPtr::acquire(|result| unsafe { SHGetPropertyStoreForWindow(window, &IPropertyStore::uuidof(), result) })
}

pub(super) fn set_window_identity(window: HWND) -> io::Result<()> {
    let store = window_store(window)?;
    let store = store.get();
    let executable = std::env::current_exe()?;
    let installed = installed_executable().is_ok_and(|p| p == executable);
    if !installed {
        let mut value: PROPVARIANT = unsafe { std::mem::zeroed() };
        value.vt = VT_BOOL as u16;
        unsafe {
            *value.data.boolVal_mut() = -1;
            shell_result(store.SetValue(&PKEY_AppUserModel_PreventPinning, &value))?;
        }
        return set_text(store, &PKEY_AppUserModel_ID, &mut wide(APP_ID));
    }
    let executable_wide: Vec<u16> = executable.as_os_str().encode_wide().collect();

    let mut command: Vec<u16> = [u16::from(b'"')].into_iter()
        .chain(executable_wide.iter().copied())
        .chain([u16::from(b'"'), 0])
        .collect();
    let mut name_resource: Vec<u16> = [u16::from(b'@')].into_iter()
        .chain(executable_wide.iter().copied())
        .chain(",-101".encode_utf16())
        .chain([0])
        .collect();
    let mut icon_resource: Vec<u16> = executable_wide
        .into_iter()
        .chain(",0".encode_utf16())
        .chain([0])
        .collect();
    set_text(store, &PKEY_AppUserModel_RelaunchCommand, &mut command)?;
    set_text(store, &PKEY_AppUserModel_RelaunchDisplayNameResource, &mut name_resource)?;
    set_text(store, &PKEY_AppUserModel_RelaunchIconResource, &mut icon_resource)?;
    set_text(store, &PKEY_AppUserModel_ID, &mut wide(APP_ID))
}

pub(super) fn clear_window_identity(window: HWND) {
    if let Ok(store) = window_store(window) {
        let empty: PROPVARIANT = unsafe { std::mem::zeroed() };
        for key in [
            &PKEY_AppUserModel_ID,
            &PKEY_AppUserModel_RelaunchCommand,
            &PKEY_AppUserModel_RelaunchDisplayNameResource,
            &PKEY_AppUserModel_RelaunchIconResource,
            &PKEY_AppUserModel_PreventPinning,
        ] {
            unsafe { store.get().SetValue(key, &empty) };
        }
    }
}

fn known_folder(folder_id: &GUID) -> io::Result<PathBuf> {
    let mut value = ptr::null_mut();
    unsafe {
        shell_result(SHGetKnownFolderPath(folder_id, 0, ptr::null_mut(), &mut value))?;
        let mut length = 0;
        while *value.add(length) != 0 {
            length += 1;
        }
        let folder = OsString::from_wide(std::slice::from_raw_parts(value, length));
        CoTaskMemFree(value.cast());
        Ok(PathBuf::from(folder))
    }
}

fn installed_executable() -> io::Result<PathBuf> {
    Ok(known_folder(&FOLDERID_LocalAppData)?.join("Programs").join("GuanhaiBafen").join(EXE_NAME))
}

fn save_shortcut(shortcut: &Path, executable: &Path, directory: &Path) -> io::Result<()> {
    let link: ComPtr<IShellLinkW> = ComPtr::acquire(|output| unsafe {
        CoCreateInstance(&SHELL_LINK, ptr::null_mut(), CLSCTX_INPROC_SERVER, &IShellLinkW::uuidof(), output)
    })?;
    let target = wide_path(executable);
    let work_dir = wide_path(directory);
    unsafe {
        shell_result(link.get().SetPath(target.as_ptr()))?;
        shell_result(link.get().SetWorkingDirectory(work_dir.as_ptr()))?;
        shell_result(link.get().SetIconLocation(target.as_ptr(), 0))?;
        shell_result(link.get().SetDescription(wide(PRODUCT_NAME).as_ptr()))?;
    }
    let properties: ComPtr<IPropertyStore> = link.query()?;
    set_text(properties.get(), &PKEY_AppUserModel_ID, &mut wide(APP_ID))?;
    shell_result(unsafe { properties.get().Commit() })?;
    let file: ComPtr<IPersistFile> = link.query()?;
    shell_result(unsafe { file.get().Save(wide_path(shortcut).as_ptr(), TRUE) })
}

pub(super) fn install(repair_shortcuts: bool) -> io::Result<PathBuf> {
    let source = std::env::current_exe()?;
    let executable = installed_executable()?;
    let directory = executable.parent().expect("installed application directory");
    let first_install = !executable.try_exists()?;
    if source != executable {
        std::fs::create_dir_all(directory)?;
        let mut staged = tempfile::NamedTempFile::new_in(directory)?;
        io::copy(&mut std::fs::File::open(source)?, staged.as_file_mut())?;
        staged.as_file().sync_all()?;
        staged.persist(&executable).map_err(|error| error.error)?;
    }
    for folder in [&FOLDERID_Desktop, &FOLDERID_Programs] {
        let shortcut = known_folder(folder)?.join(format!("{PRODUCT_NAME}.lnk"));
        if first_install || repair_shortcuts || shortcut.try_exists()? {
            save_shortcut(&shortcut, &executable, directory)?;
        }
    }
    Ok(executable)
}
