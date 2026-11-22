//! Exchange input and output with a child without filling either pipe.

use std::io::{self, Write};
use std::process::{Command, Output, Stdio};

/// Waits for the child and returns the prompt write separately from its exit.
pub fn run(command: &mut Command, input: &[u8]) -> io::Result<(Output, io::Result<()>)> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    std::thread::scope(|scope| {
        let writer =
            match std::thread::Builder::new().spawn_scoped(scope, move || stdin.write_all(input)) {
                Ok(writer) => writer,
                Err(error) => {
                    let killed = child.kill();
                    let waited = child.wait();
                    if let Err(reap_error) = waited {
                        return Err(io::Error::other(format!(
                            "{error}. Waiting for the child failed: {reap_error}"
                        )));
                    }
                    if let Err(kill_error) = killed
                        && kill_error.kind() != io::ErrorKind::InvalidInput
                    {
                        return Err(io::Error::other(format!(
                            "{error}. Stopping the child failed: {kill_error}"
                        )));
                    }
                    return Err(error);
                }
            };
        let output = child.wait_with_output();
        let written = writer
            .join()
            .map_err(|_| io::Error::other("the child input writer panicked"))?;
        output.map(|output| (output, written))
    })
}
