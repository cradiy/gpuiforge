#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct App(PathBuf);

impl App {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gpuiforge-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("sdk/platform-tools")).unwrap();
        fs::write(
            root.join("gpuiforge.json"),
            serde_json::json!({
                "app": {"name": "Log probe"},
                "platforms": {"android": {
                    "application-id": "org.forge.logprobe",
                    "template": "unused-template",
                    "management": "manual",
                    "abis": ["arm64-v8a"],
                    "logs": {"level": "warn", "tail": 37, "tags": ["Scheduler"]}
                }}
            })
            .to_string(),
        )
        .unwrap();
        let adb = root.join("sdk/platform-tools/adb");
        fs::write(&adb, r#"#!/bin/sh
printf '%s\n' "$*" >> "$PROBE_ROOT/calls"
case "$*" in
  'devices -l')
    printf 'List of devices attached\nprobe-one\tdevice model:Log_Probe\nblocked\tunauthorized\nabsent\toffline\n'
    if [ "$PROBE_STATE" = multiple ]; then printf 'probe-two\tdevice model:Other_Probe\n'; fi ;;
  *getprop*) printf 'x86_64\n' ;;
  *'shell cmd package list packages'*)
    printf 'package:org.forge.logprobe.helper uid:11970\n'
    if [ "$PROBE_STATE" != missing ]; then printf 'package:org.forge.logprobe uid:11971\n'; fi ;;
  *logcat*)
    printf '%s\n' "$$" > "$PROBE_ROOT/logcat-pid"
    if [ "$PROBE_STATE" = failed ]; then printf 'transport lost\n' >&2; exit 7; fi
    printf 'listener output\n'
    case " $* " in *' -d '*) exit 0 ;; esac
    exec sleep 30 ;;
  *) printf 'unexpected adb invocation\n' >&2; exit 8 ;;
esac
"#).unwrap();
        fs::set_permissions(adb, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gpuiforge"));
        command
            .arg("logs")
            .args(args)
            .current_dir(&self.0)
            .env("ANDROID_HOME", self.0.join("sdk"))
            .env("PROBE_ROOT", &self.0)
            .stdin(Stdio::null());
        command
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.0.join("calls")).unwrap_or_default()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Listener(Child);

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn logs_scope_to_exact_package_uid_and_apply_config_or_cli_filters() {
    let app = App::new();
    let output = app.command(&["--dump"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"listener output\n");
    let calls = app.calls();
    assert!(calls.contains(
        "-s probe-one shell cmd package list packages -U --user current org.forge.logprobe"
    ));
    assert!(calls.contains(
        "logcat --uid=11971 -b main,system,crash -v threadtime -T 37 -d -- *:S Scheduler:W"
    ));
    assert!(!calls.contains("--pid") && !calls.contains("install") && !calls.contains("start"));
    assert!(!app.0.join("target").exists());

    fs::write(app.0.join("calls"), "").unwrap();
    let output = app
        .command(&[
            "--device",
            "probe-one",
            "--dump",
            "--level",
            "error",
            "--tail",
            "64",
            "--tag",
            "WorkerQueue",
            "--tag",
            "Renderer",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let calls = app.calls();
    assert!(calls.contains("-T 64 -d -- *:S WorkerQueue:E Renderer:E"));
    assert!(!calls.contains("Scheduler"));

    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(app.0.join("gpuiforge.json")).unwrap()).unwrap();
    config["platforms"]["android"]
        .as_object_mut()
        .unwrap()
        .remove("logs");
    fs::write(app.0.join("gpuiforge.json"), config.to_string()).unwrap();
    fs::write(app.0.join("calls"), "").unwrap();
    assert!(app.command(&["--dump"]).output().unwrap().status.success());
    assert!(app.calls().contains("-T 100 -d -- *:D"));
}

#[test]
fn logs_reject_unavailable_scope_and_report_logcat_failures() {
    let app = App::new();
    for (state, args, diagnostic) in [
        ("multiple", vec!["--dump"], "--device SERIAL"),
        ("", vec!["--device", "blocked", "--dump"], "unauthorized"),
        ("", vec!["--device", "absent", "--dump"], "offline"),
        ("", vec!["--device", "unknown", "--dump"], "not connected"),
        ("missing", vec!["--dump"], "not installed"),
        ("failed", vec!["--dump"], "adb logcat failed"),
    ] {
        fs::write(app.0.join("calls"), "").unwrap();
        let output = app
            .command(&args)
            .env("PROBE_STATE", state)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if state != "failed" {
            assert!(!app.calls().contains("logcat"));
        }
    }
    let output = app
        .command(&["--device", "probe-two", "--dump"])
        .env("PROBE_STATE", "multiple")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(app.calls().contains("-s probe-two logcat"));
}

#[test]
fn logs_stream_before_exit_and_interrupt_reaps_the_listener() {
    let app = App::new();
    let stdout = fs::File::create(app.0.join("stdout")).unwrap();
    let mut listener = Listener(
        app.command(&[])
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read(app.0.join("stdout"))
            .unwrap()
            .starts_with(b"listener output\n")
        {
            break;
        }
        assert!(Instant::now() < deadline, "listener did not stream output");
        assert!(
            listener.0.try_wait().unwrap().is_none(),
            "listener exited before streaming"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(listener.0.try_wait().unwrap().is_none());
    assert!(!app.calls().contains(" -d "));
    let adb_pid = fs::read_to_string(app.0.join("logcat-pid")).unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &listener.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    loop {
        if let Some(status) = listener.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "listener did not exit on interrupt"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !Command::new("kill")
            .args(["-0", adb_pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success(),
        "ADB child survived interrupt"
    );
}
