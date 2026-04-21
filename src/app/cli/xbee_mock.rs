use super::xbee_test;
use std::process::ExitCode;

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    xbee_test::run_mock(args, bin_name)
}
