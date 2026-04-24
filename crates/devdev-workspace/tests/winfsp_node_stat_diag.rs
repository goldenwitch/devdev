//! WinFSP driver diagnosis: capture Node.js's exact stat error for a
//! WinFSP-mounted drive.
//!
//! Gated behind `DEVDEV_LIVE_COPILOT=1` (same env as the other live
//! tests — Node is available on the same machines that have Copilot
//! installed). Mounts a workspace, pre-seeds `/workspace/hello.txt`,
//! then shells out to `node -e` to run a battery of probes. Prints
//! the full Node output verbatim so we can see what `fs.statSync`
//! rejects.
#![cfg(target_os = "windows")]

use std::time::Duration;

use devdev_workspace::Workspace;

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_COPILOT")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn which_windows(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(';') {
        for ext in &[".cmd", ".bat", ".exe"] {
            let candidate = std::path::Path::new(dir).join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

#[test]
#[ignore = "requires WinFSP + node.exe; run with DEVDEV_LIVE_COPILOT=1 and --ignored"]
fn node_stat_probe_against_mounted_winfsp_drive() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    let Some(node) = which_windows("node") else {
        eprintln!("skipped: node.exe not on PATH");
        return;
    };

    let mut workspace = Workspace::new();
    let mount = workspace.mount().expect("mount winfsp drive");
    {
        let fs = workspace.fs();
        let mut g = fs.lock().unwrap();
        g.mkdir_p(b"/workspace", 0o755).unwrap();
        g.write_path(b"/workspace/hello.txt", b"hi\n").unwrap();
    }
    let mp = mount.display().to_string();
    let root = if mp.ends_with(':') { format!("{mp}\\") } else { mp.clone() };
    let subdir = format!("{root}workspace");
    let subfile = format!("{root}workspace\\hello.txt");
    eprintln!(
        "[diag] mount={root:?} subdir={subdir:?} subfile={subfile:?}"
    );

    // Run a compact Node probe. Everything captured as JSON so we
    // can grep the log for the exact failing operation + errno.
    let script = format!(
        r#"
const fs = require('fs');
function probe(label, fn) {{
  try {{
    const out = fn();
    console.log(JSON.stringify({{label, ok: true, out}}));
  }} catch (e) {{
    console.log(JSON.stringify({{
      label,
      ok: false,
      code: e.code,
      errno: e.errno,
      syscall: e.syscall,
      message: e.message,
    }}));
  }}
}}
const root = {root_json};
const sub = {sub_json};
const subfile = {subfile_json};
probe('stat C:\\\\',           () => fs.statSync('C:\\\\').isDirectory());
probe('stat root',              () => fs.statSync(root).isDirectory());
probe('lstat root',             () => fs.lstatSync(root).isDirectory());
probe('access R root',          () => {{ fs.accessSync(root, fs.constants.R_OK); return 'ok'; }});
probe('access W root',          () => {{ fs.accessSync(root, fs.constants.W_OK); return 'ok'; }});
probe('realpath root',          () => fs.realpathSync(root));
probe('readdir root',           () => fs.readdirSync(root));
probe('stat sub',               () => fs.statSync(sub).isDirectory());
probe('readdir sub',            () => fs.readdirSync(sub));
probe('stat subfile',           () => fs.statSync(subfile).size);
probe('readFile subfile',       () => fs.readFileSync(subfile).toString());
probe('process.chdir root',     () => {{ process.chdir(root); return process.cwd(); }});
probe('spawnSync cwd=root',     () => {{
  const cp = require('child_process');
  const r = cp.spawnSync(process.execPath, ['-e','console.log(process.cwd())'], {{cwd: root}});
  return {{status: r.status, stdout: r.stdout && r.stdout.toString(), stderr: r.stderr && r.stderr.toString(), error: r.error && String(r.error)}};
}});
probe('spawnSync cwd=sub',      () => {{
  const cp = require('child_process');
  const r = cp.spawnSync(process.execPath, ['-e','console.log(process.cwd())'], {{cwd: sub}});
  return {{status: r.status, stdout: r.stdout && r.stdout.toString(), stderr: r.stderr && r.stderr.toString(), error: r.error && String(r.error)}};
}});
probe('writeFile in root',      () => {{
  fs.writeFileSync(root + 'probe-write.txt', 'w');
  return fs.readFileSync(root + 'probe-write.txt').toString();
}});
probe('writeFile in root',      () => {{
  fs.writeFileSync(root + 'probe-write.txt', 'w');
  return fs.readFileSync(root + 'probe-write.txt').toString();
}});
probe('writeFile in sub',       () => {{
  fs.writeFileSync(sub + '\\\\probe-write.txt', 'w');
  return fs.readFileSync(sub + '\\\\probe-write.txt').toString();
}});
probe('DriveType root',         () => {{
  const cp = require('child_process');
  const ps = "(New-Object System.IO.DriveInfo '" + root + "').DriveType";
  const r = cp.spawnSync('powershell', ['-NoProfile','-Command', ps], {{encoding:'utf8'}});
  return {{status:r.status, stdout:(r.stdout||'').trim(), stderr:(r.stderr||'').trim()}};
}});
probe('DriveType C',            () => {{
  const cp = require('child_process');
  const ps = "(New-Object System.IO.DriveInfo 'C:\\\\').DriveType";
  const r = cp.spawnSync('powershell', ['-NoProfile','-Command', ps], {{encoding:'utf8'}});
  return {{status:r.status, stdout:(r.stdout||'').trim(), stderr:(r.stderr||'').trim()}};
}});
probe('DriveFormat root',       () => {{
  const cp = require('child_process');
  const ps = "(New-Object System.IO.DriveInfo '" + root + "').DriveFormat";
  const r = cp.spawnSync('powershell', ['-NoProfile','-Command', ps], {{encoding:'utf8'}});
  return {{status:r.status, stdout:(r.stdout||'').trim(), stderr:(r.stderr||'').trim()}};
}});
probe('realpathSync.native root', () => fs.realpathSync.native(root));
probe('realpathSync.native sub',  () => fs.realpathSync.native(sub));
// Async realpath via fs/promises — this is what Copilot CLI uses.
(async () => {{
  const fsp = require('fs/promises');
  for (const [label, p] of [['promises.realpath root', root], ['promises.realpath sub', sub]]) {{
    try {{
      const out = await fsp.realpath(p);
      console.log(JSON.stringify({{label, ok: true, out}}));
    }} catch (e) {{
      console.log(JSON.stringify({{label, ok: false, code: e.code, errno: e.errno, syscall: e.syscall, message: e.message}}));
    }}
  }}
}})();
"#,
        root_json = serde_json_str(&root),
        sub_json = serde_json_str(&subdir),
        subfile_json = serde_json_str(&subfile),
    );

    let out = std::process::Command::new(&node)
        .args(["-e", &script])
        .output()
        .expect("spawn node");
    eprintln!(
        "[diag] node exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // Give the driver a moment to drop cleanly.
    drop(workspace);
    std::thread::sleep(Duration::from_millis(200));
}

/// Minimal JSON-quote for injecting strings into a `node -e` script.
fn serde_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
