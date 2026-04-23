//! Build-time linker configuration.
//!
//! On Windows we link against the WinFSP import library (shipped
//! with the WinFSP install). The install path is discovered from the
//! `WINFSP_PATH` env var if set, or from the standard install
//! location at `C:\Program Files (x86)\WinFsp`.
//!
//! On other targets this build script is a no-op.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINFSP_PATH");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let root = std::env::var_os("WINFSP_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files (x86)\WinFsp"));

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let lib_name = match arch.as_str() {
        "x86_64" => "winfsp-x64",
        "x86" => "winfsp-x86",
        "aarch64" => "winfsp-a64",
        other => panic!("unsupported Windows arch for WinFSP link: {other}"),
    };

    let lib_dir = root.join("lib");
    if !lib_dir.join(format!("{lib_name}.lib")).exists() {
        panic!(
            "WinFSP import library not found at {}. Install WinFSP from https://winfsp.dev/ \
             or set WINFSP_PATH to the install root.",
            lib_dir.display()
        );
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib={lib_name}");
    // Delay-load the WinFSP DLL so test binaries (and any non-
    // FUSE-using binary) can be loaded even when the DLL isn't on
    // PATH. The DLL is resolved on first call, at which point
    // Windows searches PATH + the standard directories — WinFSP's
    // installer adds `C:\Program Files (x86)\WinFsp\bin` to the
    // system PATH, so resolution succeeds at use time.
    println!("cargo:rustc-link-arg=/DELAYLOAD:{lib_name}.dll");
    println!("cargo:rustc-link-lib=dylib=delayimp");
}
