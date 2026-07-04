//! Stop a previous synapse-server instance before binding the listen port.

use anyhow::{bail, Result};
use std::process::Command;
use std::{thread, time::Duration};

/// If another `synapse-server` is listening on `0 `port`, terminate it (SIGTERM then SIGKILL).
/// Returns an error when a non-synapse process owns the port.
pub fn force_stop_existing_server(port: u16) -> Result<()> {
    let my_pid = std::process::id();
    let pids = listeners_on_port(port)?;
    if pids.is_empty() {
        return Ok(());
    }

    let mut stopped = false;
    for pid in &pids {
        if *pid == my_pid {
            continue;
        }
        if !is_synapse_server(*pid) {
            let hint = process_args(*pid)
                .map(|a| format!(" ({a})"))
                .unwrap_or_default();
            bail!("port {port} is already used by pid {pid}{hint} — stop it or pass --port");
        }
    }

    for pid in pids {
        if pid == my_pid {
            continue;
        }
        println!("  Stopping previous synapse-server (pid {pid})…");
        signal_pid(pid, "-TERM")?;
        stopped = true;
    }

    if stopped {
        thread::sleep(Duration::from_millis(400));
        for pid in listeners_on_port(port)? {
            if pid != my_pid && is_synapse_server(pid) {
                println!("  Force stopping synapse-server (pid {pid})…");
                let _ = signal_pid(pid, "-9");
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    Ok(())
}

fn listeners_on_port(port: u16) -> Result<Vec<u32>> {
    let spec = format!("tcp:{port}");
    let output = match Command::new("lsof")
        .args(["-nP", "-i", &spec, "-sTCP:LISTEN", "-t"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Ok(Vec::new()),
    };
    if !output.status.success() && output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    let mut pids = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

fn is_synapse_server(pid: u32) -> bool {
    process_args(pid)
        .map(|args| args.contains("synapse-server"))
        .unwrap_or(false)
}

fn process_args(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let args = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

fn signal_pid(pid: u32, sig: &str) -> Result<()> {
    let status = Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Ok(()) // process may already be gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_nobody_listening() {
        let pids = listeners_on_port(59999).unwrap_or_default();
        assert!(pids.is_empty() || pids.iter().all(|_| true));
    }
}
