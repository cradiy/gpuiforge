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
  *'exec-out screencap -p'*)
    if [ "$PROBE_STATE" = capture-failed ]; then printf 'capture unavailable\n' >&2; exit 5; fi
    if [ "$PROBE_STATE" = capture-invalid ]; then printf 'no image\n'; exit 0; fi
    if [ "$PROBE_STATE" = capture-truncated ]; then printf '\211PNG\r\n\032\n'; exit 0; fi
    cat "$PROBE_ROOT/capture-input" ;;
  *logcat*)
    printf '%s\n' "$$" > "$PROBE_ROOT/logcat-pid"
    if [ "$PROBE_STATE" = failed ]; then printf 'transport lost\n' >&2; exit 7; fi
    case " $* " in
      *' -b crash '*) cat "$PROBE_ROOT/crash-input" || exit 6 ;;
      *) printf 'listener output\n' ;;
    esac
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

#[test]
fn crash_reports_preserve_native_and_java_stacks_without_prefix_matches() {
    let app = App::new();
    let line = |pid, tid, tag, message| {
        format!("04-16 09:13:27.321 {pid:5} {tid:5} F {tag:<14}: {message}\n")
    };
    let native_header = [
        line(840, 841, "DEBUG", "*** *** *** *** *** *** *** ***"),
        line(840, 841, "DEBUG", "Build fingerprint: 'crash-testbed'"),
    ]
    .concat();
    let native_stack = [
        line(840, 841, "DEBUG", "Cmdline: org.forge.logprobe"),
        line(
            840,
            841,
            "DEBUG",
            "pid: 620, tid: 625, name: render >>> org.forge.logprobe <<<",
        ),
        line(840, 841, "DEBUG", "uid: 11971"),
        line(840, 841, "DEBUG", "signal 6 (SIGABRT), code -1"),
        line(
            840,
            841,
            "DEBUG",
            "    #00 pc 0000000000000234 /system/lib64/libc.so",
        ),
    ]
    .concat();
    let foreign_java = [
        line(710, 712, "AndroidRuntime", "FATAL EXCEPTION: pool"),
        line(
            710,
            712,
            "AndroidRuntime",
            "Process: org.forge.logprobe.helper, PID: 710",
        ),
        line(
            710,
            712,
            "AndroidRuntime",
            "java.lang.IllegalStateException: mentions org.forge.logprobe",
        ),
    ]
    .concat();
    let java = [
        line(720, 722, "AndroidRuntime", "FATAL EXCEPTION: work"),
        line(
            720,
            722,
            "AndroidRuntime",
            "Process: org.forge.logprobe:worker, PID: 720",
        ),
        line(
            720,
            722,
            "AndroidRuntime",
            "java.lang.UnsupportedOperationException: unavailable",
        ),
        line(
            720,
            722,
            "AndroidRuntime",
            "    at runtime.Worker.execute(Worker.java:57)",
        ),
    ]
    .concat();
    let reused = [
        line(840, 841, "DEBUG", "*** *** *** *** *** *** *** ***"),
        line(
            840,
            841,
            "DEBUG",
            "pid: 630, tid: 631, name: worker >>> org.forge.logprobe.extra <<<",
        ),
        line(840, 841, "DEBUG", "foreign native stack"),
        line(720, 722, "AndroidRuntime", "FATAL EXCEPTION: worker"),
        line(
            720,
            722,
            "AndroidRuntime",
            "Process: org.forge.logprobe.other, PID: 720",
        ),
        line(720, 722, "AndroidRuntime", "foreign Java stack"),
    ]
    .concat();
    let legacy = [
        line(840, 841, "DEBUG", "*** *** *** *** *** *** *** ***"),
        line(840, 841, "DEBUG", "ABI: 'arm64'"),
        line(
            840,
            841,
            "DEBUG",
            "pid: 640, tid: 642, name: background >>> org.forge.logprobe:background <<<",
        ),
        line(
            840,
            841,
            "DEBUG",
            "    #00 pc 0000000000000468 /system/lib64/libc.so",
        ),
    ]
    .concat();
    let mut input = [
        "--------- beginning of crash\n".into(),
        line(
            810,
            811,
            "DEBUG",
            "orphan stack mentions org.forge.logprobe",
        ),
        native_header.clone(),
        foreign_java,
        native_stack.clone(),
        java.clone(),
        reused,
        legacy.clone(),
    ]
    .concat()
    .into_bytes();
    let mut binary = line(840, 841, "DEBUG", "stack byte: ").into_bytes();
    binary.pop();
    binary.push(0xff);
    input.extend(&binary);
    fs::write(app.0.join("crash-input"), input).unwrap();
    let output = app.command(&["--crash", "--dump"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected = [native_header, native_stack, java, legacy]
        .concat()
        .into_bytes();
    expected.extend(binary);
    assert_eq!(output.stdout, expected);
    let calls = app.calls();
    assert!(calls.contains("logcat -b crash -v threadtime -d -- *:V"));
    assert!(!calls.contains("--uid") && !calls.contains("--pid"));
    assert!(!calls.contains("-T") && !calls.contains("Scheduler"));
    assert!(!app.0.join("target").exists());
    fs::write(app.0.join("calls"), "").unwrap();
    assert!(
        app.command(&["--crash", "--dump", "--tail", "250"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(app.calls().contains("-T 250 -d -- *:V"));
    for args in [
        vec!["--crash", "--level", "fatal"],
        vec!["--crash", "--tag", "DEBUG"],
    ] {
        fs::write(app.0.join("calls"), "").unwrap();
        assert!(!app.command(&args).output().unwrap().status.success());
        assert!(app.calls().is_empty());
    }
}

#[test]
fn crash_streaming_flushes_before_exit_and_interrupt_reaps_the_listener() {
    let app = App::new();
    fs::write(
        app.0.join("crash-input"),
        "04-16 09:13:27.321  1010  1011 E AndroidRuntime: FATAL EXCEPTION: main\n\
         04-16 09:13:27.322  1010  1011 E AndroidRuntime: Process: org.forge.logprobe, PID: 1010\n\
         04-16 09:13:27.323  1010  1011 E AndroidRuntime: listener output\n",
    )
    .unwrap();
    follow_and_interrupt(&app, &["logs", "--crash"]);
    let calls = app.calls();
    assert!(calls.contains("-b crash"));
    assert!(!calls.contains("--uid"));
    let failed = app
        .command(&["--crash", "--dump"])
        .env("PROBE_STATE", "failed")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("adb logcat failed"));
    #[cfg(target_os = "linux")]
    {
        let full = fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut listener = Listener(
            app.command(&["--crash"])
                .stdout(full)
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = listener.0.try_wait().unwrap() {
                assert!(!status.success(), "output errors must fail the listener");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "output error did not stop logcat"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid = fs::read_to_string(app.0.join("logcat-pid")).unwrap();
        assert!(
            !Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(app.0.join("crash-input"), "").unwrap();
    let empty = app.command(&["--crash", "--dump"]).output().unwrap();
    assert!(empty.status.success());
    assert!(empty.stdout.is_empty());
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

#[test]
fn launch_uses_selected_device_without_stopping_the_application() {
    let app = App::new();
    follow_and_interrupt(&app, &["launch", "--logs"]);
    let calls = app.calls();
    let launch = calls.find("-s probe-one shell am start -W --user current -n org.forge.logprobe/org.forge.logprobe.EntryActivity").unwrap();
    let logs = calls.find("-s probe-one logcat --uid=11971").unwrap();
    assert!(launch < logs);
    assert!(!calls.contains("force-stop") && !calls.contains("install"));
    assert_eq!(
        calls.lines().filter(|line| *line == "devices -l").count(),
        1
    );
    assert!(!app.0.join("target").exists());

    fs::write(app.0.join("calls"), "").unwrap();
    let output = app
        .cli(&["launch", "--device", "probe-two"])
        .env("PROBE_STATE", "multiple")
        .output()
        .unwrap();
    assert!(output.status.success());
    let calls = app.calls();
    assert!(calls.contains("-s probe-two shell am start"));
    assert!(!calls.contains("force-stop") && !calls.contains("logcat"));

    for state in ["missing", "launch-failed"] {
        fs::write(app.0.join("calls"), "").unwrap();
        let output = app
            .cli(&["launch", "--logs"])
            .env("PROBE_STATE", state)
            .env("PROBE_LOGCAT_ONCE", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let calls = app.calls();
        assert_eq!(calls.contains("am start"), state == "launch-failed");
        assert!(!calls.contains("force-stop") && !calls.contains("logcat"));
    }
}

#[test]
fn screenshot_preserves_png_bytes_without_project_configuration() {
    let app = App::new();
    fs::remove_file(app.0.join("gpuiforge.json")).unwrap();
    let png = b"\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x02\x00\x00\x00\x01\x08\x06\x00\x00\x00\xf4\x22\x7f\x8a\x00\x00\x00\x11\x49\x44\x41\x54\x78\x9c\x63\x10\x32\x09\xfb\xbf\x6a\xf7\x19\x06\x00\x0f\x75\x03\xcd\x28\xd7\x65\x49\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";
    fs::write(app.0.join("capture-input"), png).unwrap();
    let output = app
        .cli(&["screenshot", "--config", "missing.json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert_eq!(
        saved.parent().unwrap(),
        dunce::canonicalize(&app.0).unwrap()
    );
    assert_eq!(fs::read(&saved).unwrap(), png);
    assert!(app.calls().contains("-s probe-one exec-out screencap -p"));

    fs::write(app.0.join("calls"), "").unwrap();
    let destination = app.0.join("screen selected.png");
    let output = app
        .cli(&["screenshot", "--device", "probe-two"])
        .arg(&destination)
        .env("PROBE_STATE", "multiple")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fs::read(&destination).unwrap(), png);
    assert!(app.calls().contains("-s probe-two exec-out screencap -p"));
}

#[test]
fn screenshot_failures_leave_no_image_and_preserve_existing_files() {
    let app = App::new();
    let destination = app.0.join("capture.png");
    for state in ["capture-failed", "capture-invalid", "capture-truncated"] {
        let output = app
            .cli(&["screenshot"])
            .arg(&destination)
            .env("PROBE_STATE", state)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!destination.exists());
    }
    fs::write(&destination, "keep existing content").unwrap();
    fs::write(app.0.join("calls"), "").unwrap();
    let output = app.cli(&["screenshot"]).arg(&destination).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&destination).unwrap(), b"keep existing content");
    assert!(app.calls().is_empty());
}
