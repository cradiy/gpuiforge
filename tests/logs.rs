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
                    "activity": "org.forge.logprobe.EntryActivity",
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
  *' install '*) if [ "$PROBE_STATE" = install-failed ]; then exit 4; fi ;;
  *'am force-stop'*) if [ "$PROBE_STATE" = stop-failed ]; then printf 'Error: unable to stop\n'; fi ;;
  *'am start'*) if [ "$PROBE_STATE" = launch-failed ]; then printf 'Error: unable to launch\n'; fi ;;
  *logcat*)
    printf '%s\n' "$$" > "$PROBE_ROOT/logcat-pid"
    if [ "$PROBE_STATE" = failed ]; then printf 'transport lost\n' >&2; exit 7; fi
    printf 'listener output\n'
    if [ "$PROBE_LOGCAT_ONCE" = 1 ]; then exit 0; fi
    case " $* " in *' -d '*) exit 0 ;; esac
    exec sleep 30 ;;
  *) printf 'unexpected adb invocation\n' >&2; exit 8 ;;
esac
"#).unwrap();
        fs::set_permissions(adb, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = self.cli(&["logs"]);
        command.args(args);
        command
    }

    fn cli(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gpuiforge"));
        command
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
    follow_and_interrupt(&app, &["logs"]);
}

fn follow_and_interrupt(app: &App, args: &[&str]) {
    let stdout = fs::File::create(app.0.join("stdout")).unwrap();
    let mut listener = Listener(
        app.cli(args)
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if String::from_utf8_lossy(&fs::read(app.0.join("stdout")).unwrap())
            .contains("listener output\n")
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

#[test]
fn run_logs_preserves_steps_and_reuses_selected_device() {
    let app = App::new();
    fs::create_dir(app.0.join("native")).unwrap();
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let android = &mut config["platforms"]["android"];
    android["abis"] = serde_json::json!(["x86_64"]);
    android["project-dir"] = "native".into();
    android["artifact"] = "probe.apk".into();
    android["build"] = serde_json::json!([{
        "program": "sh",
        "args": ["-c", "printf 'build:%s\\n' \"$1\" >> \"$PROBE_ROOT/calls\"; printf package > \"$PROBE_ROOT/native/probe.apk\"", "build-probe", "{{abis}}"]
    }]);
    android["run"] = serde_json::json!([
        {"program": "{{adb}}", "args": ["-s", "{{device}}", "install", "-r", "{{artifact}}"]},
        {"program": "sh", "args": ["-c", "printf 'custom-run\\n' >> \"$PROBE_ROOT/calls\""]},
        {"program": "{{adb}}", "args": ["-s", "{{device}}", "shell", "am", "start", "-W", "-n", "{{application_id}}/{{activity}}"], "error-pattern": "Error:"}
    ]);
    fs::write(&config_path, config.to_string()).unwrap();
    follow_and_interrupt(&app, &["run", "--logs"]);
    let calls = app.calls();
    assert_eq!(
        calls.lines().filter(|line| *line == "devices -l").count(),
        1
    );
    let build = calls.find("build:x86_64").unwrap();
    let install = calls.find("-s probe-one install -r").unwrap();
    let custom = calls.find("custom-run").unwrap();
    let launch = calls.find("-s probe-one shell am start").unwrap();
    let logs = calls.find("-s probe-one logcat --uid=11971").unwrap();
    assert!(build < install && install < custom && custom < launch && launch < logs);
    assert!(calls.contains("-T 37 -- *:S Scheduler:W"));

    for (failure, launched) in [("install-failed", false), ("launch-failed", true)] {
        fs::write(app.0.join("calls"), "").unwrap();
        let output = app
            .cli(&["run", "--logs", "--device", "probe-one"])
            .env("PROBE_STATE", failure)
            .env("PROBE_LOGCAT_ONCE", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let calls = app.calls();
        assert_eq!(calls.contains("am start"), launched);
        assert!(!calls.contains("logcat"));
    }
    config["platforms"]["desktop"] = serde_json::json!({
        "run": [{"program": "sh", "args": ["-c", "printf executed > desktop-ran"]}]
    });
    fs::write(&config_path, config.to_string()).unwrap();
    let output = app.cli(&["run", "desktop", "--logs"]).output().unwrap();
    assert!(!output.status.success());
    assert!(!app.0.join("desktop-ran").exists());
}

#[test]
fn restart_uses_installed_app_without_build_or_abi_selection() {
    let app = App::new();
    follow_and_interrupt(&app, &["restart", "--logs"]);
    let calls = app.calls();
    assert_eq!(
        calls.lines().filter(|line| *line == "devices -l").count(),
        1
    );
    let stop = calls
        .find("-s probe-one shell am force-stop --user current org.forge.logprobe")
        .unwrap();
    let start = calls.find("-s probe-one shell am start -W --user current -n org.forge.logprobe/org.forge.logprobe.EntryActivity").unwrap();
    let logs = calls.find("-s probe-one logcat --uid=11971").unwrap();
    assert!(stop < start && start < logs);
    assert!(!calls.contains("install") && !app.0.join("target").exists());

    fs::write(app.0.join("calls"), "").unwrap();
    assert!(app.cli(&["restart"]).output().unwrap().status.success());
    assert!(!app.calls().contains("logcat"));
    for (failure, stopped, launched) in [
        ("missing", false, false),
        ("stop-failed", true, false),
        ("launch-failed", true, true),
    ] {
        fs::write(app.0.join("calls"), "").unwrap();
        let output = app
            .cli(&["restart", "--logs"])
            .env("PROBE_STATE", failure)
            .env("PROBE_LOGCAT_ONCE", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let calls = app.calls();
        assert_eq!(calls.contains("am force-stop"), stopped);
        assert_eq!(calls.contains("am start"), launched);
        assert!(!calls.contains("logcat"));
    }
}
