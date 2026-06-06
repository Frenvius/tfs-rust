use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Run on a thread with a larger stack to accommodate Lua + map/item loading
    // which can have deep call chains in debug mode.
    let result = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || tfs_rust::run(args))
        .expect("failed to spawn main thread")
        .join()
        .expect("main thread panicked");

    match result {
        Ok(tfs_rust::ExitStatus::Success) => ExitCode::SUCCESS,
        Ok(tfs_rust::ExitStatus::Failure) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}
