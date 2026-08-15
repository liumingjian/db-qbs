use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_dir = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("source crate must live under the workspace root");

    for path in [
        "package.json",
        "package-lock.json",
        "tsconfig.json",
        "vite.config.ts",
        "web/index.html",
        "web/src",
        "docs/design-system/tokens.css",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            workspace_dir.join(path).display()
        );
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(npm)
        .args(["run", "build", "--silent"])
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|error| {
            panic!("could not run the web build; run `npm install` first: {error}")
        });
    assert!(status.success(), "web build failed with status {status}");
}
