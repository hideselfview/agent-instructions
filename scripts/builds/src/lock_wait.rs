//! How long cargo waited for its file locks. When another cargo holds a lock
//! that a build needs (its build folder, the package cache), cargo prints a
//! line that starts `Blocking waiting for file lock` on stderr, and prints its
//! next line once it holds the lock. `relay` passes cargo's stderr through and
//! adds up the time from each such line to the line after it.

use std::io::{BufRead, BufReader, Read, Write};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::clock::Moment;

/// The words cargo starts a line with while it waits for a file lock.
const BLOCKING: &[u8] = b"Blocking waiting for file lock";

/// Copies every line of <from> to <to> as it arrives, and returns, once <from>
/// ends, how long cargo waited for its file locks.
pub fn relay(
    from: impl Read + Send + 'static,
    mut to: impl Write + Send + 'static,
) -> JoinHandle<Duration> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from);
        let mut waited = Duration::ZERO;
        let mut blocked: Option<Moment> = None;
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    crate::output::note(format!("reading cargo's messages stopped: {error}"));
                    break;
                }
            }
            // If writing to stderr fails, nobody can read the messages, so
            // relay ignores the error.
            let _ = to.write_all(&line);
            let blocking = line
                .windows(BLOCKING.len())
                .any(|window| window == BLOCKING);
            match (blocking, blocked) {
                (true, None) => blocked = Some(Moment::now()),
                (false, Some(since)) => {
                    waited += since.elapsed();
                    blocked = None;
                }
                _ => {}
            }
        }
        if let Some(since) = blocked {
            waited += since.elapsed();
        }
        waited
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that gives its lines one read at a time and takes <pause>
    /// before the line after a blocking one, as cargo does while it waits.
    struct Cargo {
        lines: Vec<&'static str>,
        pause: Duration,
    }

    impl Read for Cargo {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.lines.is_empty() {
                return Ok(0);
            }
            let line = self.lines.remove(0);
            if line.contains("Compiling") {
                std::thread::sleep(self.pause);
            }
            buf[..line.len()].copy_from_slice(line.as_bytes());
            Ok(line.len())
        }
    }

    #[derive(Clone, Default)]
    struct Kept(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Kept {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn counts_the_time_from_a_blocking_line_to_the_next_and_passes_every_line_on() {
        let lines = vec![
            "    Blocking waiting for file lock on build directory\n",
            "   Compiling demo v0.1.0\n",
            "    Finished `dev` profile\n",
        ];
        let kept = Kept::default();
        let waited = relay(
            Cargo {
                lines: lines.clone(),
                pause: Duration::from_millis(200),
            },
            kept.clone(),
        )
        .join()
        .unwrap();
        assert!(waited >= Duration::from_millis(200), "{waited:?}");
        assert_eq!(
            String::from_utf8(kept.0.lock().unwrap().clone()).unwrap(),
            lines.concat()
        );
        let never = relay(
            Cargo {
                lines: vec!["   Compiling demo v0.1.0\n", "    Finished\n"],
                pause: Duration::ZERO,
            },
            Kept::default(),
        )
        .join()
        .unwrap();
        assert_eq!(never, Duration::ZERO);
    }
}
