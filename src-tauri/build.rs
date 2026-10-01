use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
  tauri_build::build();
  embed_test_manifest();
}

/// The unit-test harness links the same system libraries as the app binary --
/// including comctl32 -- but unlike the app binary it receives no application
/// manifest. Without a manifest Windows binds comctl32 v5, which lacks the
/// v6-only exports the image imports (TaskDialogIndirect, SetWindowSubclass,
/// DefSubclassProc, RemoveWindowSubclass), so the loader refuses the test
/// binary with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before main() runs.
///
/// Fix: embed a minimal manifest declaring the Common-Controls v6 side-by-side
/// dependency into test targets only (the app binary already gets its manifest
/// from tauri-build). The manifest is compiled to a COFF object with windres
/// and injected via cargo:rustc-link-arg-tests.
fn embed_test_manifest() {
  if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
    return;
  }
  if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "gnu" {
    // MSVC-hosted test binaries would embed the manifest through the
    // /MANIFEST linker flag instead; that toolchain is not used here.
    return;
  }
  let windres = match ["x86_64-w64-mingw32-windres", "windres"].into_iter().find(|name| on_path(name)) {
    Some(tool) => tool,
    None => {
      println!("cargo:warning=windres.exe not found on PATH; unit-test binaries will fail to load (missing Common-Controls manifest)");
      return;
    }
  };
  let windows_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("windows");
  let object = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("test_manifest.o");
  let status = Command::new(windres)
    .current_dir(&windows_dir)
    .args(["--input", "test-manifest.rc", "--output"])
    .arg(&object)
    .status()
    .unwrap_or_else(|error| panic!("failed to run {windres}: {error}"));
  if !status.success() {
    panic!("{windres} failed to compile windows/test-manifest.rc");
  }
  println!("cargo:rustc-link-arg-tests={}", object.display());
  println!("cargo:rerun-if-changed=windows/test-manifest.rc");
  println!("cargo:rerun-if-changed=windows/test-manifest.xml");
}

fn on_path(name: &str) -> bool {
  let path = env::var("PATH").unwrap_or_default();
  env::split_paths(&path)
    .map(|directory| directory.join(format!("{name}.exe")))
    .any(|candidate| candidate.is_file())
}

