use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Always,
    Auto,
    Never,
}

#[derive(Debug)]
pub enum Event {
    Started {
        index: usize,
    },
    Output {
        index: usize,
        bytes: Vec<u8>,
    },
    Finished {
        index: usize,
        result: CommandResult,
        elapsed: Duration,
    },
    AllFinished {
        cancelled: bool,
    },
}

#[derive(Debug)]
pub enum CommandResult {
    Exited(ExitStatus),
    SpawnFailed(String),
    Cancelled,
}

pub fn run(
    commands: Vec<String>,
    color: ColorMode,
    sender: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    for (index, command_line) in commands.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let started_at = Instant::now();
        let _ = sender.send(Event::Started { index });

        let mut command = shell_command(&command_line);
        configure_color(&mut command, color);
        // collapse owns the terminal for navigation. Letting a child read that terminal while it
        // lives in a separate process group can suspend it with SIGTTIN and stall the whole queue.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = sender.send(Event::Finished {
                    index,
                    result: CommandResult::SpawnFailed(error.to_string()),
                    elapsed: started_at.elapsed(),
                });
                continue;
            }
        };

        let stdout_reader = child
            .stdout
            .take()
            .map(|stdout| pipe_reader(stdout, index, sender.clone()));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stderr| pipe_reader(stderr, index, sender.clone()));

        let (status, was_cancelled) = loop {
            if cancel.load(Ordering::Relaxed) {
                terminate(&mut child);
                break (child.wait().ok(), true);
            }

            match child.try_wait() {
                Ok(Some(status)) => break (Some(status), false),
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break (child.wait().ok(), false),
            }
        };

        if let Some(reader) = stdout_reader {
            reader.finish();
        }
        if let Some(reader) = stderr_reader {
            reader.finish();
        }

        let result = if was_cancelled {
            CommandResult::Cancelled
        } else if let Some(status) = status {
            CommandResult::Exited(status)
        } else {
            CommandResult::SpawnFailed("could not read the command's exit status".into())
        };

        let _ = sender.send(Event::Finished {
            index,
            result,
            elapsed: started_at.elapsed(),
        });

        if was_cancelled {
            break;
        }
    }

    let _ = sender.send(Event::AllFinished {
        cancelled: cancel.load(Ordering::Relaxed),
    });
}

struct PipeReader {
    handle: thread::JoinHandle<()>,
    done: Receiver<()>,
}

impl PipeReader {
    fn finish(self) {
        // Some commands start daemons that accidentally retain the stdout/stderr pipe. Waiting
        // forever for EOF would prevent the next command from starting, even though the command
        // itself has exited. Give the reader time to drain normal buffered output, then detach it.
        if self.done.recv_timeout(Duration::from_millis(250)).is_ok() {
            let _ = self.handle.join();
        }
    }
}

fn pipe_reader<R: Read + Send + 'static>(
    mut stream: R,
    index: usize,
    sender: Sender<Event>,
) -> PipeReader {
    let (done_sender, done) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender
                        .send(Event::Output {
                            index,
                            bytes: buffer[..count].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = done_sender.send(());
    });
    PipeReader { handle, done }
}

#[cfg(unix)]
fn shell_command(command_line: &str) -> Command {
    use std::os::unix::process::CommandExt as _;

    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(command_line).process_group(0);
    command
}

#[cfg(windows)]
fn shell_command(command_line: &str) -> Command {
    let mut command = Command::new("cmd");
    command.arg("/C").arg(command_line);
    command
}

#[cfg(unix)]
fn terminate(child: &mut std::process::Child) {
    // The shell starts in its own process group, so a negative PID kills its whole command tree.
    // SAFETY: `kill` accepts any process-group ID; failure (including a stale PID) is harmless here.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate(child: &mut std::process::Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

fn configure_color(command: &mut Command, color: ColorMode) {
    match color {
        ColorMode::Always => {
            command.env_remove("NO_COLOR");
            command.env("CLICOLOR_FORCE", "1");
            command.env("FORCE_COLOR", "3");
            command.env("CARGO_TERM_COLOR", "always");
            command.env("COLORTERM", "truecolor");
            if std::env::var_os("TERM").is_none() {
                command.env("TERM", "xterm-256color");
            }
        }
        ColorMode::Auto => {}
        ColorMode::Never => {
            command.env("NO_COLOR", "1");
            command.env("CLICOLOR", "0");
            command.env("CLICOLOR_FORCE", "0");
            command.env("FORCE_COLOR", "0");
            command.env("CARGO_TERM_COLOR", "never");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn runs_commands_in_order_and_captures_output() {
        let (sender, receiver) = mpsc::channel();
        run(
            vec!["printf first".into(), "printf second".into()],
            ColorMode::Auto,
            sender,
            Arc::new(AtomicBool::new(false)),
        );

        let events: Vec<_> = receiver.into_iter().collect();
        let starts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::Started { index } => Some(*index),
                _ => None,
            })
            .collect();
        let output: Vec<u8> = events
            .iter()
            .filter_map(|event| match event {
                Event::Output { bytes, .. } => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();

        assert_eq!(starts, [0, 1]);
        assert_eq!(String::from_utf8(output).unwrap(), "firstsecond");
        assert!(matches!(events.last(), Some(Event::AllFinished { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn advances_when_a_background_process_retains_the_output_pipes() {
        let (sender, receiver) = mpsc::channel();
        let started_at = Instant::now();
        let worker = thread::spawn(move || {
            run(
                vec!["sleep 3 & printf first".into(), "printf second".into()],
                ColorMode::Auto,
                sender,
                Arc::new(AtomicBool::new(false)),
            );
        });

        let mut second_started = false;
        while started_at.elapsed() < Duration::from_secs(2) {
            if matches!(
                receiver.recv_timeout(Duration::from_millis(100)),
                Ok(Event::Started { index: 1 })
            ) {
                second_started = true;
                break;
            }
        }
        worker.join().unwrap();

        assert!(
            second_started,
            "the retained pipe blocked the next command from starting"
        );
    }

    #[test]
    fn cancellation_stops_a_command_tree() {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::spawn(move || {
            run(
                vec![long_running_command().into()],
                ColorMode::Auto,
                sender,
                worker_cancel,
            );
        });

        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Started { .. })
        ));
        thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut finished = false;
        while Instant::now() < deadline {
            if matches!(
                receiver.recv_timeout(Duration::from_millis(100)),
                Ok(Event::AllFinished { cancelled: true })
            ) {
                finished = true;
                break;
            }
        }
        worker.join().unwrap();
        assert!(finished, "runner did not stop after cancellation");
    }

    #[cfg(unix)]
    fn long_running_command() -> &'static str {
        "sleep 10 | cat"
    }

    #[cfg(windows)]
    fn long_running_command() -> &'static str {
        "ping -n 11 127.0.0.1 > nul"
    }
}
