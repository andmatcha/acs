use std::env;
use std::process::Command;

const UNKNOWN: &str = "unknown";

fn main() {
    for key in [
        "ACS_BUILD_BRANCH",
        "ACS_BUILD_COMMIT",
        "ACS_BUILD_DIRTY",
        "ACS_BUILD_SOURCE_KIND",
        "ACS_BUILD_SOURCE_REF",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let commit = env_or_git("ACS_BUILD_COMMIT", &["rev-parse", "HEAD"])
        .unwrap_or_else(|| UNKNOWN.to_owned());
    let branch = env::var("ACS_BUILD_BRANCH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_branch)
        .unwrap_or_else(|| UNKNOWN.to_owned());
    let dirty = env::var("ACS_BUILD_DIRTY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if git_is_dirty() {
                String::from("dirty")
            } else {
                String::from("clean")
            }
        });
    let source_kind = env::var("ACS_BUILD_SOURCE_KIND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if dirty == "dirty" {
                String::from("working-tree")
            } else {
                String::from("local-commit")
            }
        });
    let source_ref = env::var("ACS_BUILD_SOURCE_REF")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if branch != UNKNOWN {
                branch.clone()
            } else {
                String::from(UNKNOWN)
            }
        });

    println!("cargo:rustc-env=ACS_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=ACS_BUILD_BRANCH={branch}");
    println!("cargo:rustc-env=ACS_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=ACS_BUILD_SOURCE_KIND={source_kind}");
    println!("cargo:rustc-env=ACS_BUILD_SOURCE_REF={source_ref}");
}

fn env_or_git(key: &str, args: &[&str]) -> Option<String> {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(args))
}

fn git_branch() -> Option<String> {
    let branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        Some(String::from("detached"))
    } else {
        Some(branch)
    }
}

fn git_is_dirty() -> bool {
    git_output(&["status", "--porcelain"])
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false)
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    (!trimmed.is_empty()).then_some(trimmed.to_owned())
}
