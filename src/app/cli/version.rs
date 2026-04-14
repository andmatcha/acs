use std::process::ExitCode;

const BUILD_BRANCH: &str = env!("ACS_BUILD_BRANCH");
const BUILD_COMMIT: &str = env!("ACS_BUILD_COMMIT");
const BUILD_DIRTY: &str = env!("ACS_BUILD_DIRTY");
const BUILD_SOURCE_KIND: &str = env!("ACS_BUILD_SOURCE_KIND");
const BUILD_SOURCE_REF: &str = env!("ACS_BUILD_SOURCE_REF");
const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const UNKNOWN: &str = "unknown";

pub(crate) fn print_version() -> ExitCode {
    println!("{}", version_line());
    ExitCode::SUCCESS
}

pub(crate) fn version_line() -> String {
    let mut details = Vec::new();

    if BUILD_COMMIT != UNKNOWN {
        details.push(format!("commit {}", short_commit(BUILD_COMMIT)));
    }
    if BUILD_BRANCH != UNKNOWN {
        details.push(format!("branch {BUILD_BRANCH}"));
    }
    details.push(format_source(
        BUILD_SOURCE_KIND,
        BUILD_SOURCE_REF,
        BUILD_BRANCH,
    ));
    details.push(BUILD_DIRTY.to_owned());

    format!("{PACKAGE_NAME} {PACKAGE_VERSION} ({})", details.join(", "))
}

fn format_source(source_kind: &str, source_ref: &str, branch: &str) -> String {
    if source_kind == UNKNOWN {
        return String::from("source unknown");
    }

    if source_ref == UNKNOWN || source_ref == branch {
        return format!("source {source_kind}");
    }

    format!("source {source_kind} {source_ref}")
}

fn short_commit(commit: &str) -> &str {
    let short_len = commit.len().min(12);
    &commit[..short_len]
}

#[cfg(test)]
mod tests {
    use super::{format_source, short_commit};

    #[test]
    fn format_source_omits_duplicate_branch_ref() {
        assert_eq!(
            format_source("local-commit", "main", "main"),
            "source local-commit"
        );
    }

    #[test]
    fn format_source_keeps_distinct_ref() {
        assert_eq!(
            format_source("remote-tag", "v0.1.0", "detached"),
            "source remote-tag v0.1.0"
        );
    }

    #[test]
    fn short_commit_limits_to_twelve_chars() {
        assert_eq!(short_commit("1234567890abcdef"), "1234567890ab");
        assert_eq!(short_commit("1234567"), "1234567");
    }
}
