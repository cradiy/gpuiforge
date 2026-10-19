use crate::devices;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn save(requested: Option<&str>, destination: Option<&Path>) -> Result<PathBuf> {
    let destination = match destination {
        Some(path) => path.to_path_buf(),
        None => {
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
            PathBuf::from(format!(
                "screenshot-{}-{:09}.png",
                timestamp.as_secs(),
                timestamp.subsec_nanos()
            ))
        }
    };
    let destination = std::env::current_dir()?.join(destination);
    let filename = destination
        .file_name()
        .context("screenshot destination must name a file")?;
    let parent = crate::host::canonicalize(
        destination
            .parent()
            .context("screenshot destination has no parent")?,
    )?;
    ensure!(
        parent.is_dir(),
        "screenshot destination parent must be a directory"
    );
    let destination = parent.join(filename);
    ensure!(
        !destination.exists(),
        "screenshot destination already exists: {}",
        destination.display()
    );

    let device = devices::select_for_logs(requested)?;
    eprintln!("Capturing screen on {device}");
    let output = Command::new(devices::adb())
        .args(["-s", &device, "exec-out", "screencap", "-p"])
        .stdin(Stdio::null())
        .output()
        .context("starting adb screen capture")?;
    ensure!(
        output.status.success(),
        "adb screen capture failed: {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    io::stderr().write_all(&output.stderr)?;
    ensure!(
        output.stdout.starts_with(b"\x89PNG\r\n\x1a\n")
            && output.stdout.ends_with(b"\0\0\0\0IEND\xae\x42\x60\x82"),
        "adb screen capture did not return a complete PNG"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .with_context(|| format!("creating screenshot {}", destination.display()))?;
    let result = file.write_all(&output.stdout).and_then(|()| file.flush());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&destination);
        return Err(error).with_context(|| format!("saving screenshot {}", destination.display()));
    }
    Ok(destination)
}
