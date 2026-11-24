use en3_recovery::modern::{decrypt_file_monitored, encrypt_file_monitored};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_output_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = PathBuf::from("target")
        .join("test-output")
        .join(format!("monitor-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn monitored_encryption_reports_all_input_bytes() {
    let directory = test_output_dir();
    let input = directory.join("input.bin");
    let archive = directory.join("archive.bfq");
    let data = vec![42_u8; 100_000];
    fs::write(&input, &data).unwrap();
    let mut last = 0;
    encrypt_file_monitored(&input, "password", Some(&archive), |processed| {
        last = processed;
        Ok(())
    })
    .unwrap();
    assert_eq!(last, data.len() as u64);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cancellation_removes_partial_output() {
    let directory = test_output_dir();
    let input = directory.join("input.bin");
    let archive = directory.join("archive.bfq");
    let recovered = directory.join("recovered.bin");
    fs::write(&input, vec![42_u8; 100_000]).unwrap();
    let error = encrypt_file_monitored(&input, "password", Some(&archive), |processed| {
        if processed >= 65_536 {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!archive.exists());

    encrypt_file_monitored(&input, "password", Some(&archive), |_| Ok(())).unwrap();
    let error = decrypt_file_monitored(&archive, "password", Some(&recovered), |_| {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    })
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!recovered.exists());
    fs::remove_dir_all(directory).unwrap();
}
