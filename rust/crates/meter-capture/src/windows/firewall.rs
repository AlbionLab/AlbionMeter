//! `WindowsFirewall`: an inbound allow rule for the meter, so the raw socket sees incoming game traffic.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RULE_NAME: &str = "Albion Meter (packet capture)";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirewallRule {
    /// The rule was already there.
    Present,
    Added(String),
    Failed,
}

impl FirewallRule {
    /// The line the C# server printed, if any.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Present => None,
            Self::Added(program) => Some(format!(
                "  Firewall: added an inbound rule for {program} so packet capture sees incoming game traffic."
            )),
            Self::Failed => {
                Some("  Firewall: the inbound rule for packet capture could not be added.".into())
            }
        }
    }
}

/// `None` when netsh could not run at all.
pub fn allow_capture() -> Option<FirewallRule> {
    let program = std::env::current_exe().ok()?;
    let program = program.to_string_lossy();
    if run(&format!(
        "advfirewall firewall show rule name=\"{RULE_NAME}\""
    ))? == 0
    {
        return Some(FirewallRule::Present);
    }
    let added = run(&format!(
        "advfirewall firewall add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{program}\" enable=yes profile=any"
    ))?;
    Some(if added == 0 {
        FirewallRule::Added(program.into_owned())
    } else {
        FirewallRule::Failed
    })
}

/// netsh's exit code, -1 when it does not finish within 10 s.
fn run(arguments: &str) -> Option<i32> {
    let mut child = Command::new("netsh")
        .raw_arg(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return Some(-1);
            }
        }
    }
}
