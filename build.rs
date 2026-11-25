use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=windows/guanhai.rc");
    println!("cargo:rerun-if-changed=desktop/src-tauri/icons/icon.ico");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let sdk_rc = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .and_then(|program_files| std::fs::read_dir(program_files.join("Windows Kits/10/bin")).ok())
        .and_then(|versions| {
            versions
                .filter_map(Result::ok)
                .map(|version| version.path().join("x64/rc.exe"))
                .filter(|candidate| candidate.is_file())
                .max()
        });
    let compiler = env::var_os("RC")
        .map(PathBuf::from)
        .or(sdk_rc)
        .unwrap_or_else(|| PathBuf::from("rc.exe"));
    let resource = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("guanhai.res");
    let status = Command::new(compiler)
        .args(["/nologo", "/fo"])
        .arg(&resource)
        .arg("windows/guanhai.rc")
        .status()
        .expect("run Windows SDK resource compiler");
    assert!(status.success(), "compile Windows application resources");
    println!("cargo:rustc-link-arg-bin=guanhai-win7={}", resource.display());
}
