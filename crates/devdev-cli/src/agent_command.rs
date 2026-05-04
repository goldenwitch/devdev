//! Resolve a user-supplied agent program (`copilot`, `./copilot.cmd`,
//! some custom binary) into a `(program, args)` pair that
//! [`tokio::process::Command::new`] can spawn reliably across hosts.
//!
//! The resolution is the **single canonical entry-point** for every
//! call site that spawns the ACP agent subprocess (the daemon's
//! [`crate::acp_backend`], live integration tests, ad-hoc PoCs).
//! Splitting this responsibility was costing us:
//!
//! 1. Bare `Command::new("copilot")` on Windows fails because the
//!    OS resolver doesn't apply `PATHEXT` to `CreateProcess` for
//!    extensionless names — every caller has to expand to
//!    `copilot.cmd` itself.
//! 2. The Copilot CLI's `copilot(.cmd|.exe)` launcher invokes a
//!    Node SEA prebuilt that ignores `NODE_OPTIONS=--require`, so
//!    our WinFSP realpath shim never reaches the process that
//!    handles `session/new`. We work around it by invoking
//!    `node <copilot>/index.js` directly. That rewrite needs the
//!    *resolved* launcher path, not the bare name, so it must live
//!    after the PATH search.
//!
//! The pipeline is:
//!
//! ```text
//! prepare(program, args)
//!   = rewrite_copilot_sea_launcher(  // Windows-only Node-SEA bypass
//!       resolve_on_path(program),    // PATH + PATHEXT lookup
//!       args
//!     )
//! ```
//!
//! Both steps are no-ops when not relevant. On non-Windows hosts the
//! function is a thin pass-through.

use std::path::{Path, PathBuf};

/// Resolve the agent program and apply any Windows-specific launch
/// rewrites. See module docs for the rationale.
///
/// On a missing executable the original `program` is returned
/// unchanged and `Command::spawn` will be the one that fails — that
/// keeps the error surface in one well-known place (the spawn site)
/// rather than splitting it between resolve-time and spawn-time.
pub fn prepare(program: &str, args: &[String]) -> (String, Vec<String>) {
    let resolved = resolve_on_path(program).unwrap_or_else(|| program.to_string());
    if let Some(rewritten) = rewrite_copilot_sea_launcher(&resolved, args) {
        rewritten
    } else {
        (resolved, args.to_vec())
    }
}

/// Walk `PATH` (and `PATHEXT` on Windows) to find an executable whose
/// stem matches `program`. Returns the absolute path on success.
///
/// If `program` already contains a path separator or an extension,
/// it's treated as a path: returned verbatim if it exists, else
/// `None`.
pub fn resolve_on_path(program: &str) -> Option<String> {
    let prog_path = Path::new(program);
    let has_separator = program.contains('/') || program.contains('\\');
    let has_extension = prog_path.extension().is_some();
    if has_separator || has_extension {
        return prog_path
            .is_file()
            .then(|| prog_path.to_string_lossy().into_owned());
    }

    let path_var = std::env::var_os("PATH")?;
    let exts = path_extensions();

    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate: PathBuf = if ext.is_empty() {
                dir.join(program)
            } else {
                dir.join(format!("{program}{ext}"))
            };
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

#[cfg(windows)]
fn path_extensions() -> Vec<String> {
    // PATHEXT is `.COM;.EXE;.BAT;.CMD;...`. Lowercase + strip leading
    // dot so we can format `{program}{ext}` cleanly with the dot.
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let mut exts: Vec<String> = raw
        .split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    // Also try the bare name in case the user registered a script
    // with no extension.
    exts.push(String::new());
    exts
}

#[cfg(not(windows))]
fn path_extensions() -> Vec<String> {
    // Unix: no PATHEXT; the binary is named exactly as written.
    vec![String::new()]
}

/// Rewrite a `(program, args)` pair that launches Copilot via its
/// `copilot(.cmd|.ps1|.exe)` launcher so that it invokes
/// `node <copilot>/index.js` directly instead.
///
/// Why: Copilot's launcher runs `npm-loader.js`, which `spawnSync`s
/// the platform-specific **Node SEA** prebuilt (`@github/copilot-
/// win32-x64/copilot.exe`) as the actual agent process. Node SEAs
/// intentionally ignore `NODE_OPTIONS=--require <path>` as a
/// security measure, so our WinFSP realpath shim never reaches the
/// process that handles `session/new` — meaning Copilot rejects
/// every WinFSP cwd.
///
/// Invoking `node index.js --acp ...` directly keeps all behaviour
/// identical (index.js sees `--acp` and imports `app.js` in-process,
/// which is the same code path the SEA runs) but preserves our
/// NODE_OPTIONS injection. Returns `None` on non-Windows hosts or
/// when the program is not recognizable as a Copilot launcher.
pub fn rewrite_copilot_sea_launcher(
    program: &str,
    args: &[String],
) -> Option<(String, Vec<String>)> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let prog_path = Path::new(program);
    let stem = prog_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if stem.as_deref() != Some("copilot") {
        return None;
    }
    let parent = prog_path.parent().filter(|p| !p.as_os_str().is_empty())?;
    let index_js = parent
        .join("node_modules")
        .join("@github")
        .join("copilot")
        .join("index.js");
    if !index_js.is_file() {
        tracing::warn!(
            target: "devdev_cli::agent_command",
            expected = %index_js.display(),
            "copilot index.js not found next to launcher; leaving invocation as-is"
        );
        return None;
    }
    let node_exe = parent.join("node.exe");
    let node = if node_exe.is_file() {
        node_exe.display().to_string()
    } else {
        "node".to_string()
    };
    let mut new_args = Vec::with_capacity(args.len() + 1);
    new_args.push(index_js.display().to_string());
    new_args.extend(args.iter().cloned());
    tracing::info!(
        target: "devdev_cli::agent_command",
        node = %node,
        index_js = %index_js.display(),
        "bypassing Copilot SEA launcher so NODE_OPTIONS=--require <shim> applies"
    );
    Some((node, new_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_returns_none_for_clearly_missing_binary() {
        assert!(resolve_on_path("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn resolve_passes_through_existing_path_with_separator() {
        // Use a path that's guaranteed to exist on every supported host.
        let exists = if cfg!(windows) {
            "C:/Windows/System32/cmd.exe"
        } else {
            "/bin/sh"
        };
        let resolved = resolve_on_path(exists).expect("path exists");
        assert!(
            resolved.eq_ignore_ascii_case(exists)
                || resolved.replace('\\', "/").eq_ignore_ascii_case(exists)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rewrite_ignores_non_copilot_program() {
        assert!(rewrite_copilot_sea_launcher("C:/Windows/System32/cmd.exe", &[]).is_none());
        assert!(rewrite_copilot_sea_launcher("node.exe", &[]).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rewrite_returns_none_when_index_missing() {
        assert!(rewrite_copilot_sea_launcher("C:/Windows/System32/copilot.exe", &[]).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rewrite_returns_none_when_program_has_no_parent() {
        // Bare "copilot" used to slip past the parent check via
        // `prog_path.parent() == Some("")`; the explicit empty filter
        // is what makes this case a clean None now.
        assert!(rewrite_copilot_sea_launcher("copilot", &[]).is_none());
    }

    #[test]
    fn prepare_falls_through_when_program_unresolvable() {
        let (prog, args) = prepare("definitely-not-a-real-binary-xyz", &["--foo".into()]);
        assert_eq!(prog, "definitely-not-a-real-binary-xyz");
        assert_eq!(args, vec!["--foo".to_string()]);
    }
}
