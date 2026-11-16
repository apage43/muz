//! Records the source commit, which output fingerprints include: a render
//! cached by one engine build is not trusted by another.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no", "--", "src", "std"])
        .is_some_and(|s| !s.is_empty());
    println!(
        "cargo:rustc-env=MUZ_BUILD_COMMIT={commit}{}",
        if dirty { "-dirty" } else { "" }
    );
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
    }
    if let Some(dir) = git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={dir}/packed-refs");
        println!("cargo:rerun-if-changed={dir}/refs/heads");
    }
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "wavpack")]
    if let Err(e) = pkg_config::Config::new().atleast_version("5.0").probe("wavpack") {
        panic!("feature `wavpack` needs libwavpack 5 and its pkg-config file (install wavpack, or build with --no-default-features): {e}");
    }
}
