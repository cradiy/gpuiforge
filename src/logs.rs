use crate::{config::Project, devices};
use anyhow::{Context, Result, ensure};
use clap::{Args, ValueEnum};
use serde::Deserialize;
use std::{
    io::{self, IsTerminal},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

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
    /// Logcat history limit (default: configured tail or 100).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    tail: Option<u32>,
    /// Print buffered logs and exit instead of following.
    #[arg(long)]
    dump: bool,
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
    let output = Command::new(devices::adb())
        .args([
            "-s",
            &device,
            "shell",
            "cmd",
            "package",
            "list",
            "packages",
            "-U",
            "--user",
            "current",
            application,
        ])
        .output()
        .context("querying the installed Android application UID")?;
    ensure!(
        output.status.success(),
        "Android package query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packages =
        String::from_utf8(output.stdout).context("invalid Android package query output")?;
    let uid = packages
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()?.strip_prefix("package:")? != application {
                return None;
            }
            fields.find_map(|field| field.strip_prefix("uid:")?.parse::<u32>().ok())
        })
        .with_context(|| {
            format!("{application} is not installed for the current Android user on {device}")
        })?;

    let mut command = Command::new(devices::adb());
    command.args([
        "-s",
        &device,
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
    if options.dump {
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
        if options.dump { "Dumping" } else { "Following" },
        if options.dump { "" } else { "; Ctrl+C to stop" },
    );
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .context("installing the log interrupt handler")?;
    command.stdin(Stdio::null());
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
    loop {
        if interrupted.load(Ordering::Relaxed) {
            let _ = child.kill();
            child.wait().context("waiting for adb logcat to stop")?;
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                ensure!(status.success(), "adb logcat failed: {status}");
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("waiting for adb logcat");
            }
        }
    }
}
