//! Env sanitization integration test (Linux only).
//!
//! Validates that `Workspace::exec` launches children under a curated
//! environment: only a small, known set of variables should reach the
//! child process. Anything outside the allow-list is reported as a leak.

#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::ffi::OsStr;

use devdev_workspace::Workspace;

fn workspace_with_basics() -> Workspace {
    let mut ws = Workspace::new();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.mkdir_p(b"/home/agent", 0o755).unwrap();
        g.mkdir_p(b"/home/agent/.cargo", 0o755).unwrap();
    }
    ws.mount().expect("mount");
    ws
}

#[test]
fn env_is_curated() {
    // Allow-list:
    //   HOME, CARGO_HOME, USER, LOGNAME, SHELL, TERM, PATH  -- curated
    //                                                          env from
    //                                                          Workspace::exec
    //   SHLVL, PWD, _, OLDPWD                               -- PTY /
    //                                                          shell
    //                                                          injected
    //                                                          niceties
    // `/usr/bin/env` itself injects nothing; anything else is a leak
    // from the host.
    let allow: BTreeSet<&str> = [
        "HOME",
        "CARGO_HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TERM",
        "PATH",
        "SHLVL",
        "PWD",
        "_",
        "OLDPWD",
    ]
    .into_iter()
    .collect();

    let ws = workspace_with_basics();
    let mut out = Vec::new();
    let code = ws
        .exec(OsStr::new("/usr/bin/env"), &[], b"/home/agent", &mut out)
        .expect("exec");
    let dump = String::from_utf8_lossy(&out).to_string();
    assert_eq!(code, 0, "env exited {code}; output:\n{dump}");

    // Parse KEY=VALUE lines (only first `=` separates).
    let mut seen: std::collections::BTreeMap<String, String> = Default::default();
    for line in dump.lines() {
        if let Some((k, v)) = line.split_once('=') {
            seen.insert(k.to_string(), v.to_string());
        }
    }

    assert_eq!(
        seen.get("HOME").map(String::as_str),
        Some("/home/agent"),
        "HOME missing or wrong; full env:\n{dump}"
    );
    assert_eq!(
        seen.get("CARGO_HOME").map(String::as_str),
        Some("/home/agent/.cargo"),
        "CARGO_HOME missing or wrong; full env:\n{dump}"
    );
    let path = seen
        .get("PATH")
        .unwrap_or_else(|| panic!("PATH missing; full env:\n{dump}"));
    assert!(
        !path.is_empty(),
        "PATH present but empty; full env:\n{dump}"
    );

    let leaks: Vec<&String> = seen
        .keys()
        .filter(|k| !allow.contains(k.as_str()))
        .collect();
    assert!(
        leaks.is_empty(),
        "unexpected env vars leaked into child: {leaks:?}\nfull env dump:\n{dump}"
    );
}
