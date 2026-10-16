//! Login passwords kept as internet passwords in the user's Keychain.

use std::io::Write as _;
use std::process::{Command, Stdio};

const SECURITY: &str = "/usr/bin/security";

pub fn find(host: &str, user: &str) -> Option<String> {
    let output = Command::new(SECURITY)
        .args([
            "find-internet-password",
            "-r",
            "ssh ",
            "-s",
            host,
            "-a",
            user,
            "-w",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let password = String::from_utf8(output.stdout).ok()?;
    Some(password.strip_suffix('\n').unwrap_or(&password).to_owned())
}

pub fn store(host: &str, user: &str, password: &str) -> bool {
    if password.contains('\n') {
        return false;
    }
    let label = format!("{host} (Found'er)");
    let child = Command::new(SECURITY)
        .args([
            "add-internet-password",
            "-U",
            "-r",
            "ssh ",
            "-s",
            host,
            "-a",
            user,
            "-l",
            &label,
            "-w",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    let written = child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{password}\n{password}\n").as_bytes());
    child.wait().is_ok_and(|status| status.success()) && written.is_ok()
}

pub fn forget(host: &str, user: &str) -> bool {
    run(&[
        "delete-internet-password",
        "-r",
        "ssh ",
        "-s",
        host,
        "-a",
        user,
    ])
}

fn run(args: &[&str]) -> bool {
    Command::new(SECURITY)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
