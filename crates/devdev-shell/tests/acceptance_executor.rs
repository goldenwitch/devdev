//! Acceptance tests for capability 09 — shell executor / `ShellSession`.
//!
//! Uses lightweight fake `ToolEngine` and `VirtualGit` backends so each
//! test runs in milliseconds without touching Wasmtime or libgit2.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use devdev_git::{GitResult, VirtualGit};
use devdev_shell::{ShellSession, parse};
use devdev_vfs::MemFs;
use devdev_wasm::{ToolEngine, ToolResult};

// ── Fakes ────────────────────────────────────────────────────────────────

/// A minimal tool engine that understands a handful of commands used in
/// these tests (`cat`, `echo`, `wc`, `grep`, `false`, `true`, `env`, `pass`).
/// Everything else returns 127 `command not found`.
struct FakeTools;

impl ToolEngine for FakeTools {
    fn execute(
        &self,
        command: &str,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        _cwd: &str,
        fs: &mut MemFs,
    ) -> ToolResult {
        match command {
            "echo" => ToolResult {
                stdout: format!("{}\n", args.join(" ")).into_bytes(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            "cat" => {
                let mut out = Vec::new();
                if args.is_empty() {
                    out.extend_from_slice(stdin);
                } else {
                    for a in args {
                        let abs = std::path::PathBuf::from(if a.starts_with('/') {
                            a.clone()
                        } else {
                            format!("/{a}")
                        });
                        match fs.read(&abs) {
                            Ok(data) => out.extend_from_slice(&data),
                            Err(e) => {
                                return ToolResult {
                                    stdout: Vec::new(),
                                    stderr: format!("cat: {a}: {e}\n").into_bytes(),
                                    exit_code: 1,
                                };
                            }
                        }
                    }
                }
                ToolResult {
                    stdout: out,
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            "wc" => {
                // -l → count lines in stdin
                let count = if args.iter().any(|a| a == "-l") {
                    stdin.iter().filter(|&&b| b == b'\n').count()
                } else {
                    stdin.len()
                };
                ToolResult {
                    stdout: format!("{count}\n").into_bytes(),
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            "grep" => {
                let pattern = args.iter().find(|a| !a.starts_with('-')).cloned();
                let Some(pat) = pattern else {
                    return ToolResult {
                        stdout: Vec::new(),
                        stderr: b"grep: missing pattern\n".to_vec(),
                        exit_code: 2,
                    };
                };
                let text = String::from_utf8_lossy(stdin);
                let mut out = String::new();
                for line in text.lines() {
                    if line.contains(&pat) {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                let code = if out.is_empty() { 1 } else { 0 };
                ToolResult {
                    stdout: out.into_bytes(),
                    stderr: Vec::new(),
                    exit_code: code,
                }
            }
            "true" => ToolResult {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            "false" => ToolResult {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 1,
            },
            "env" => {
                let mut keys: Vec<&String> = env.keys().collect();
                keys.sort();
                let mut out = String::new();
                for k in keys {
                    out.push_str(&format!("{k}={}\n", env[k]));
                }
                ToolResult {
                    stdout: out.into_bytes(),
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            "pass" => ToolResult {
                stdout: stdin.to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            other => ToolResult {
                stdout: Vec::new(),
                stderr: format!("command not found: {other}\n").into_bytes(),
                exit_code: 127,
            },
        }
    }

    fn available_tools(&self) -> Vec<&str> {
        vec!["cat", "echo", "wc", "grep", "false", "true", "env", "pass"]
    }

    fn has_tool(&self, name: &str) -> bool {
        self.available_tools().contains(&name)
    }
}

/// Fake git backend — records the last invocation and returns a canned reply.
struct FakeGit {
    reply: Mutex<Option<(Vec<String>, String)>>, // (last_args, cwd)
}

impl FakeGit {
    fn new() -> Self {
        Self {
            reply: Mutex::new(None),
        }
    }
}

impl VirtualGit for FakeGit {
    fn execute(&self, args: &[String], cwd: &str) -> GitResult {
        *self.reply.lock().unwrap() = Some((args.to_vec(), cwd.to_owned()));
        GitResult::ok(format!("git-fake: {}\n", args.join(" ")).into_bytes())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn new_session() -> ShellSession {
    let vfs = Arc::new(Mutex::new(MemFs::new()));
    let tools: Arc<dyn ToolEngine> = Arc::new(FakeTools);
    let git: Arc<Mutex<dyn VirtualGit>> = Arc::new(Mutex::new(FakeGit::new()));
    ShellSession::new(vfs, tools, git)
}

fn new_session_with_vfs() -> (ShellSession, Arc<Mutex<MemFs>>) {
    let vfs = Arc::new(Mutex::new(MemFs::new()));
    let tools: Arc<dyn ToolEngine> = Arc::new(FakeTools);
    let git: Arc<Mutex<dyn VirtualGit>> = Arc::new(Mutex::new(FakeGit::new()));
    let session = ShellSession::new(vfs.clone(), tools, git);
    (session, vfs)
}

fn stdout(r: &devdev_shell::ShellResult) -> String {
    String::from_utf8_lossy(&r.stdout).into_owned()
}

fn stderr(r: &devdev_shell::ShellResult) -> String {
    String::from_utf8_lossy(&r.stderr).into_owned()
}

// ── Acceptance Criteria ──────────────────────────────────────────────────

/// AC: `execute("echo hello")` → stdout `"hello\n"`, exit 0.
#[test]
fn echo_hello_world() {
    let mut s = new_session();
    let r = s.execute("echo hello");
    assert_eq!(stdout(&r), "hello\n");
    assert_eq!(r.exit_code, 0);
}

/// AC: `execute("cat file.txt | grep pattern | wc -l")` → correct count.
#[test]
fn pipeline_buffers_through_stages() {
    let (mut s, vfs) = new_session_with_vfs();
    vfs.lock()
        .unwrap()
        .write(
            Path::new("/file.txt"),
            b"alpha\nbeta matches\ngamma\ndelta matches\n",
        )
        .unwrap();
    let r = s.execute("cat /file.txt | grep matches | wc -l");
    assert_eq!(stdout(&r).trim(), "2");
    assert_eq!(r.exit_code, 0);
}

/// AC: `execute("grep foo > out.txt")` — final stdout written to VFS, empty result stdout.
#[test]
fn stdout_redirect_to_vfs() {
    let (mut s, vfs) = new_session_with_vfs();
    vfs.lock()
        .unwrap()
        .write(Path::new("/in.txt"), b"foo one\nbar two\nfoo three\n")
        .unwrap();
    let r = s.execute("cat /in.txt | grep foo > /out.txt");
    assert!(r.stdout.is_empty(), "stdout should be drained by redirect");
    let written = vfs.lock().unwrap().read(Path::new("/out.txt")).unwrap();
    assert_eq!(String::from_utf8(written).unwrap(), "foo one\nfoo three\n");
}

/// AC: `execute("echo $HOME")` with env `HOME=/sandbox` → `/sandbox\n`.
#[test]
fn variable_expansion() {
    let mut s = new_session();
    s.state_mut()
        .env
        .insert("HOME".into(), "/sandbox".into());
    let r = s.execute("echo $HOME");
    assert_eq!(stdout(&r), "/sandbox\n");
}

/// AC: `execute("echo $?")` after a failed command → prints previous exit code.
#[test]
fn last_exit_code_substitution() {
    let mut s = new_session();
    s.execute("false");
    let r = s.execute("echo $?");
    assert_eq!(stdout(&r), "1\n");
}

/// AC: `execute("echo *.md")` with VFS containing `/a.md`, `/b.md` → both filenames.
#[test]
fn glob_expansion_against_vfs() {
    let (mut s, vfs) = new_session_with_vfs();
    {
        let mut v = vfs.lock().unwrap();
        v.write(Path::new("/a.md"), b"").unwrap();
        v.write(Path::new("/b.md"), b"").unwrap();
        v.write(Path::new("/c.txt"), b"").unwrap();
    }
    let r = s.execute("echo *.md");
    let out = stdout(&r);
    assert!(out.contains("/a.md"), "expected /a.md in: {out:?}");
    assert!(out.contains("/b.md"), "expected /b.md in: {out:?}");
    assert!(!out.contains("/c.txt"));
}

/// AC: `execute("false && echo nope")` → echo not executed, exit 1.
#[test]
fn and_short_circuits_on_failure() {
    let mut s = new_session();
    let r = s.execute("false && echo nope");
    assert!(
        !stdout(&r).contains("nope"),
        "echo should not run: {:?}",
        stdout(&r)
    );
    assert_eq!(r.exit_code, 1);
}

/// AC: `execute("false || echo yep")` → echo runs, stdout `yep\n`.
#[test]
fn or_runs_on_failure() {
    let mut s = new_session();
    let r = s.execute("false || echo yep");
    assert_eq!(stdout(&r), "yep\n");
    assert_eq!(r.exit_code, 0);
}

/// AC: `execute("cmd1 ; cmd2")` → both run regardless of exit codes.
#[test]
fn semicolon_runs_both() {
    let mut s = new_session();
    let r = s.execute("false ; echo two");
    assert!(stdout(&r).contains("two"));
    assert_eq!(r.exit_code, 0, "last cmd was echo");
}

/// AC: `execute("git log --oneline -3")` dispatches to VirtualGit.
#[test]
fn git_dispatches_to_virtual_git() {
    let mut s = new_session();
    let r = s.execute("git log --oneline -3");
    assert!(stdout(&r).starts_with("git-fake: log --oneline -3"));
    assert_eq!(r.exit_code, 0);
}

/// AC: `execute("cd /tmp && pwd")` → cwd changed, stdout `/tmp\n`.
#[test]
fn cd_then_pwd() {
    let (mut s, vfs) = new_session_with_vfs();
    vfs.lock()
        .unwrap()
        .tree_mut()
        .insert(
            std::path::PathBuf::from("/tmp"),
            devdev_vfs::types::Node::Directory {
                mode: 0o755,
                modified: std::time::SystemTime::now(),
            },
        );
    let r = s.execute("cd /tmp && pwd");
    assert_eq!(stdout(&r), "/tmp\n");
    assert_eq!(s.cwd(), Path::new("/tmp"));
}

/// AC: `execute("exit 42")` → `session_ended: true`, exit 42.
#[test]
fn exit_ends_session() {
    let mut s = new_session();
    let r = s.execute("exit 42");
    assert!(r.session_ended);
    assert_eq!(r.exit_code, 42);
}

/// AC: parse error surfaces as stderr + exit 2.
#[test]
fn parse_error_returns_exit_2() {
    let mut s = new_session();
    let r = s.execute("echo 'unterminated");
    assert_eq!(r.exit_code, 2);
    assert!(stderr(&r).contains("parse error"), "stderr: {:?}", stderr(&r));
}

/// AC: `FOO=bar env` — env sees `FOO=bar` only for that command.
#[test]
fn per_command_env_assignment() {
    let mut s = new_session();
    let r = s.execute("FOO=bar env");
    assert!(
        stdout(&r).contains("FOO=bar"),
        "expected FOO=bar in:\n{}",
        stdout(&r)
    );
    // And NOT persisted into the session env.
    assert!(!s.env().contains_key("FOO"));
}

// ── Extras: more coverage of dispatch + redirect paths ───────────────────

/// Unknown command propagates 127 from the tool engine.
#[test]
fn unknown_command_returns_127() {
    let mut s = new_session();
    let r = s.execute("nosuch");
    assert_eq!(r.exit_code, 127);
    assert!(stderr(&r).contains("command not found"));
}

/// `>>` appends to an existing VFS file.
#[test]
fn append_redirect_extends_file() {
    let (mut s, vfs) = new_session_with_vfs();
    vfs.lock()
        .unwrap()
        .write(Path::new("/log.txt"), b"first\n")
        .unwrap();
    s.execute("echo second >> /log.txt");
    let contents = vfs.lock().unwrap().read(Path::new("/log.txt")).unwrap();
    assert_eq!(String::from_utf8(contents).unwrap(), "first\nsecond\n");
}

/// `< file` feeds VFS contents as stdin to the stage.
#[test]
fn input_redirect_reads_vfs() {
    let (mut s, vfs) = new_session_with_vfs();
    vfs.lock()
        .unwrap()
        .write(Path::new("/in.txt"), b"a\nb\nc\n")
        .unwrap();
    let r = s.execute("wc -l < /in.txt");
    assert_eq!(stdout(&r).trim(), "3");
}

/// An empty input parses cleanly and returns exit 0.
#[test]
fn empty_input_is_noop() {
    let mut s = new_session();
    let r = s.execute("");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.is_empty());
}

/// Sanity: parser + executor agree on what a pipeline is.
#[test]
fn parse_then_execute_round_trip() {
    let cmd = "echo x | pass";
    let list = parse(cmd).unwrap();
    assert_eq!(list.first.stages.len(), 2);
    let mut s = new_session();
    let r = s.execute(cmd);
    assert_eq!(stdout(&r), "x\n");
}
