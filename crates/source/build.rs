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
    let status = match Command::new(npm)
        .args(["run", "build", "--silent"])
        .current_dir(workspace_dir)
        .status()
    {
        Ok(status) => status,
        // The M1 rig builds the workspace inside `rust:1-bookworm`, which has no npm. That
        // build only needs db-qbs-source-run and db-qbs-sink, neither of which serves the
        // web assets — so an already-built dist is enough to let the crate compile.
        Err(error) => {
            let prebuilt = workspace_dir.join("web/dist/index.html");
            assert!(
                prebuilt.is_file(),
                "could not run the web build ({error}) and {} is missing; \
                 run `npm install && npm run build` on a host that has npm first",
                prebuilt.display()
            );
            println!(
                "cargo:warning=npm is unavailable ({error}); reusing the existing web/dist"
            );
            return;
        }
    };
    assert!(status.success(), "web build failed with status {status}");
}
