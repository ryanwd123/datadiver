use std::process::{Command, Stdio};

/// Start `program` with `args` without waiting for it to finish.
pub fn spawn_program(program: &str, args: &[String]) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // The app has no console, so console programs would otherwise flash a
        // new window. `cmd /C start ...` still opens one deliberately.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    // Reap the process so it does not linger as a zombie on Unix.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_report_an_error() {
        let error = spawn_program("datadiver-missing-program", &[]).unwrap_err();
        assert!(error.contains("datadiver-missing-program"));
    }
}
