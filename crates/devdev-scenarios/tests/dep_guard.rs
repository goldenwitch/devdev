//! Guardrail: the scenarios crate must depend ONLY on user-surface
//! types. Engine internals (`devdev-acp`) must never be direct
//! deps. If it leaks in as a direct dep, the scenarios crate has
//! stopped being a user-surface harness and is peeking at
//! implementation — which is exactly the drift this meta-test
//! exists to catch.
//!
//! Transitive deps are fine (devdev-daemon itself depends on
//! devdev-workspace etc.). This test only inspects the top level.

use std::process::Command;

use devdev_scenarios::workspace_root;

const FORBIDDEN: &[&str] = &["devdev-acp"];

#[test]
fn no_engine_crates_as_direct_deps() {
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .arg("-p")
        .arg("devdev-scenarios")
        .arg("--edges=normal")
        .arg("--depth=1")
        .arg("--prefix=none")
        .current_dir(workspace_root())
        .output()
        .expect("cargo tree");

    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // `cargo tree --depth=1 --prefix=none` emits one line per direct
    // dep, formatted `<name> v<version>`. The root crate is the first
    // line; everything else is a direct dep.
    let violations: Vec<&str> = stdout
        .lines()
        .skip(1) // root
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            FORBIDDEN
                .iter()
                .any(|bad| line.starts_with(&format!("{bad} ")))
        })
        .collect();

    assert!(
        violations.is_empty(),
        "devdev-scenarios has forbidden engine-crate direct deps: {violations:?}\n\
         Full tree:\n{stdout}"
    );
}
