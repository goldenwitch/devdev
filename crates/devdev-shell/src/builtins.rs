//! Shell builtins — commands that operate directly on session state.

use std::path::{Path, PathBuf};

use devdev_vfs::MemFs;

use crate::state::ShellState;

/// The result of attempting a builtin command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinResult {
    /// Command executed successfully.
    Ok {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        exit_code: i32,
    },
    /// Session should end with the given exit code.
    Exit(i32),
    /// Not a builtin — fall through to tool dispatch.
    NotBuiltin,
}

impl BuiltinResult {
    fn ok(stdout: Vec<u8>) -> Self {
        Self::Ok {
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        }
    }

    fn err(stderr: Vec<u8>) -> Self {
        Self::Ok {
            stdout: Vec::new(),
            stderr,
            exit_code: 1,
        }
    }
}

/// Try to execute a command as a shell builtin.
///
/// Returns `BuiltinResult::NotBuiltin` if the command name isn't a builtin.
pub fn try_builtin(
    name: &str,
    args: &[String],
    state: &mut ShellState,
    vfs: &MemFs,
) -> BuiltinResult {
    match name {
        "cd" => builtin_cd(args, state, vfs),
        "pwd" => builtin_pwd(state),
        "export" => builtin_export(args, state),
        "unset" => builtin_unset(args, state),
        "echo" => builtin_echo(args),
        "exit" => builtin_exit(args),
        _ => BuiltinResult::NotBuiltin,
    }
}

fn builtin_cd(args: &[String], state: &mut ShellState, vfs: &MemFs) -> BuiltinResult {
    let target = match args.first().map(|s| s.as_str()) {
        None => PathBuf::from("/"),
        Some("-") => match &state.oldpwd {
            Some(old) => old.clone(),
            None => {
                return BuiltinResult::err(b"cd: OLDPWD not set\n".to_vec());
            }
        },
        Some(path) => {
            let resolved = devdev_vfs::path::resolve(Path::new(path), &state.cwd);
            devdev_vfs::path::normalize(&resolved)
        }
    };

    // Validate path exists and is a directory
    match vfs.stat(&target) {
        Ok(stat) => {
            if stat.file_type != devdev_vfs::FileType::Directory {
                let msg = format!("cd: not a directory: {}\n", target.display());
                return BuiltinResult::err(msg.into_bytes());
            }
        }
        Err(_) => {
            let msg = format!("cd: no such file or directory: {}\n", args[0]);
            return BuiltinResult::err(msg.into_bytes());
        }
    }

    let old = state.cwd.clone();
    state.cwd = target;
    state.oldpwd = Some(old);
    BuiltinResult::ok(Vec::new())
}

fn builtin_pwd(state: &ShellState) -> BuiltinResult {
    let mut out = state.cwd.to_string_lossy().into_owned().into_bytes();
    out.push(b'\n');
    BuiltinResult::ok(out)
}

fn builtin_export(args: &[String], state: &mut ShellState) -> BuiltinResult {
    if args.is_empty() {
        // List all env vars
        let mut out = String::new();
        let mut keys: Vec<&String> = state.env.keys().collect();
        keys.sort();
        for key in keys {
            let val = &state.env[key];
            out.push_str(&format!("declare -x {key}=\"{val}\"\n"));
        }
        return BuiltinResult::ok(out.into_bytes());
    }

    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            state.env.insert(name.to_owned(), value.to_owned());
        }
        // `export FOO` without `=` is a no-op in our model
    }
    BuiltinResult::ok(Vec::new())
}

fn builtin_unset(args: &[String], state: &mut ShellState) -> BuiltinResult {
    for name in args {
        state.env.remove(name);
    }
    BuiltinResult::ok(Vec::new())
}

fn builtin_echo(args: &[String]) -> BuiltinResult {
    let mut suppress_newline = false;
    let mut start = 0;

    if args.first().map(|s| s.as_str()) == Some("-n") {
        suppress_newline = true;
        start = 1;
    }

    let text = args[start..].join(" ");
    let mut out = text.into_bytes();
    if !suppress_newline {
        out.push(b'\n');
    }
    BuiltinResult::ok(out)
}

fn builtin_exit(args: &[String]) -> BuiltinResult {
    let code = args
        .first()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    BuiltinResult::Exit(code)
}
