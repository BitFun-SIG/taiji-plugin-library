#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let Some(run_id) = arguments.next() else {
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        return ExitCode::from(2);
    }
    let Some(run_id) = run_id.to_str() else {
        return ExitCode::from(2);
    };

    match openbitfun_data_migrator_lib::run(run_id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(1),
    }
}
