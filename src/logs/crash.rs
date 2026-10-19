use std::{
    collections::VecDeque,
    io::{self, BufRead, Write},
};

struct Report {
    pid: u32,
    tid: u32,
    java: bool,
    selected: Option<bool>,
    pending: VecDeque<Vec<u8>>,
}

fn field(text: &str) -> Option<(&str, &str)> {
    let (field, rest) = text.trim_start().split_once(char::is_whitespace)?;
    Some((field, rest.trim_start()))
}

fn record(line: &str) -> Option<(u32, u32, bool, &str)> {
    let (_, rest) = field(line)?;
    let (_, rest) = field(rest)?;
    let (pid, rest) = field(rest)?;
    let (tid, rest) = field(rest)?;
    let (priority, rest) = field(rest)?;
    if !matches!(priority, "V" | "D" | "I" | "W" | "E" | "F") {
        return None;
    }
    let (tag, message) = rest.split_once(':')?;
    let java = match tag.trim() {
        "AndroidRuntime" => true,
        "DEBUG" => false,
        _ => return None,
    };
    Some((pid.parse().ok()?, tid.parse().ok()?, java, message.trim()))
}

fn process(message: &str, java: bool) -> Option<&str> {
    if java {
        message
            .strip_prefix("Process: ")?
            .split_once(", PID:")
            .map(|(name, _)| name)
    } else if let Some(command) = message.strip_prefix("Cmdline: ") {
        command.split_whitespace().next()
    } else if message.starts_with("pid: ") {
        message.split_once(" >>> ")?.1.strip_suffix(" <<<")
    } else {
        None
    }
}

pub fn filter(
    mut input: impl BufRead,
    application: &str,
    mut output: impl Write,
) -> io::Result<()> {
    let mut reports = VecDeque::<Report>::new();
    loop {
        let mut line = Vec::new();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        let Some((pid, tid, java, message)) = record(&text) else {
            continue;
        };
        let mut report = reports
            .iter()
            .position(|report| report.pid == pid && report.tid == tid && report.java == java)
            .and_then(|index| reports.remove(index))
            .unwrap_or(Report {
                pid,
                tid,
                java,
                selected: None,
                pending: VecDeque::new(),
            });
        let starts_report = if java {
            message.starts_with("FATAL EXCEPTION:")
                || message.starts_with("*** FATAL EXCEPTION IN SYSTEM PROCESS:")
        } else {
            message.starts_with("*** *** ***")
        };
        if starts_report {
            report.selected = None;
            report.pending.clear();
        }
        if let Some(name) = process(message, java) {
            report.selected = Some(
                name == application
                    || name
                        .strip_prefix(application)
                        .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1),
            );
            if report.selected == Some(true) {
                for pending in report.pending.drain(..) {
                    output.write_all(&pending)?;
                }
            } else {
                report.pending.clear();
            }
        }
        match report.selected {
            Some(true) => {
                output.write_all(&line)?;
                output.flush()?;
            }
            Some(false) => {}
            None => {
                if report.pending.len() == 32 {
                    report.pending.pop_front();
                }
                report.pending.push_back(line);
            }
        }
        if reports.len() == 64 {
            reports.pop_front();
        }
        reports.push_back(report);
    }
}
