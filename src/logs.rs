use crate::{config::Project, devices};
use anyhow::{Context, Result, ensure};
use clap::{Args, ValueEnum};
use serde::Deserialize;
use std::{
    io::{self, BufReader, IsTerminal},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod crash;

#[derive(Clone, Copy, Default, Deserialize, schemars::JsonSchema, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Verbose,
    #[default]
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    fn priority(self) -> char {
        match self {
            Self::Verbose => 'V',
            Self::Debug => 'D',
            Self::Info => 'I',
            Self::Warn => 'W',
            Self::Error => 'E',
            Self::Fatal => 'F',
        }
    }
}

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// Minimum log priority. Defaults to debug.
    pub level: LogLevel,
    /// Positive logcat history limit before following, or when dumping. Defaults to 100. Filtering may produce fewer lines.
    #[schemars(range(min = 1))]
    pub tail: u32,
    /// Exact tags to include. Empty selects all tags. CLI --tag values replace this list. Tags must be nonempty and contain no whitespace, control characters, colons or asterisks.
    pub tags: Vec<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Debug,
            tail: 100,
            tags: Vec::new(),
        }
    }
}

impl LogConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.tail > 0, "logs.tail must be positive");
        for tag in &self.tags {
            validate_tag(tag).map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}

#[derive(Args)]
pub struct LogOptions {
    /// Select a device without prompting.
    #[arg(long)]
    device: Option<String>,
    /// Minimum log priority (default: configured level or debug).
    #[arg(long, value_enum)]
    level: Option<LogLevel>,
    /// Include only this exact tag; repeat to select multiple tags.
    #[arg(long = "tag", value_parser = validate_tag)]
    tags: Vec<String>,
    /// Logcat history limit (ordinary logs: configured tail or 100; crash: all).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    tail: Option<u32>,
    /// Print buffered logs and exit instead of following.
    #[arg(long)]
    dump: bool,
    /// Show Java/native crash reports for this application's process names.
    #[arg(long, conflicts_with_all = ["level", "tags"])]
    crash: bool,
}

fn validate_tag(tag: &str) -> std::result::Result<String, String> {
    if tag.is_empty()
        || tag
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, ':' | '*'))
    {
        return Err(
            "log tags must be nonempty and contain no whitespace, control characters, ':' or '*'"
                .into(),
        );
    }
    Ok(tag.into())
}

pub fn run(project: &Project, options: &LogOptions) -> Result<()> {
    let platform = project.platform("android")?;
    let mut config = platform.logs.clone().unwrap_or_default();
    if let Some(level) = options.level {
        config.level = level;
    }
    if let Some(tail) = options.tail {
        config.tail = tail;
    }
    if !options.tags.is_empty() {
        config.tags.clone_from(&options.tags);
    }
    config.validate()?;
    let device = devices::select_for_logs(options.device.as_deref())?;
    let application = platform
        .application_id
        .as_deref()
        .context("android.application-id is required")?;
    let uid = devices::application_uid(&device, application)?;
    if options.crash {
        return crashes(&device, application, options.tail, options.dump);
    }
    stream(&config, &device, application, uid, options.dump)
}

pub fn follow(project: &Project, device: &str, uid: u32) -> Result<()> {
    let platform = project.platform("android")?;
    stream(
        &platform.logs.clone().unwrap_or_default(),
        device,
        platform
            .application_id
            .as_deref()
            .context("android.application-id is required")?,
        uid,
        false,
    )
}

fn stream(config: &LogConfig, device: &str, application: &str, uid: u32, dump: bool) -> Result<()> {
    config.validate()?;

    let mut command = Command::new(devices::adb());
    command.args([
        "-s",
        device,
        "logcat",
        &format!("--uid={uid}"),
        "-b",
        "main,system,crash",
        "-v",
        "threadtime",
    ]);
    if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        command.args(["-v", "color"]);
    }
    command.args(["-T", &config.tail.to_string()]);
    if dump {
        command.arg("-d");
    }
    command.arg("--");
    if config.tags.is_empty() {
        command.arg(format!("*:{}", config.level.priority()));
    } else {
        command.arg("*:S");
        for tag in &config.tags {
            command.arg(format!("{tag}:{}", config.level.priority()));
        }
    }
    eprintln!(
        "{} {application} on {device} (UID {uid}){}",
        if dump { "Dumping" } else { "Following" },
        if dump { "" } else { "; Ctrl+C to stop" },
    );
    listen(command, None)
}

fn crashes(device: &str, application: &str, tail: Option<u32>, dump: bool) -> Result<()> {
    let mut command = Command::new(devices::adb());
    command.args(["-s", device, "logcat", "-b", "crash", "-v", "threadtime"]);
    if let Some(tail) = tail {
        command.args(["-T", &tail.to_string()]);
    }
    if dump {
        command.arg("-d");
    }
    command.args(["--", "*:V"]);
    eprintln!(
        "{} crash reports for {application} on {device}{}",
        if dump { "Dumping" } else { "Following" },
        if dump { "" } else { "; Ctrl+C to stop" },
    );
    listen(command, Some(application))
}

fn listen(mut command: Command, crash_application: Option<&str>) -> Result<()> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .context("installing the log interrupt handler")?;
    command.stdin(Stdio::null());
    if crash_application.is_some() {
        command.stdout(Stdio::piped());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn().context("starting adb logcat")?;
    let output_failed = Arc::new(AtomicBool::new(false));
    let output = crash_application.map(|application| {
        let application = application.to_owned();
        let stdout = child.stdout.take().unwrap();
        let failed = Arc::clone(&output_failed);
        std::thread::spawn(move || {
            let result = crash::filter(BufReader::new(stdout), &application, io::stdout().lock());
            if result.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            result
        })
    });
    let result = loop {
        if interrupted.load(Ordering::Relaxed) || output_failed.load(Ordering::Relaxed) {
            let _ = child.kill();
            break child
                .wait()
                .context("waiting for adb logcat to stop")
                .map(|_| ());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("adb logcat failed: {status}"))
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error).context("waiting for adb logcat");
            }
        }
    };
    if let Some(output) = output {
        output
            .join()
            .map_err(|_| anyhow::anyhow!("crash log reader panicked"))?
            .context("filtering crash reports")?;
    }
    result
}
