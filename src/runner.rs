use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Always,
    Auto,
    Never,
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl TerminalSize {
    fn as_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
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
    Exited { code: u32, signal: Option<String> },
    SpawnFailed(String),
    Cancelled,
}

pub fn run(
    commands: Vec<String>,
    color: ColorMode,
    terminal_size: TerminalSize,
    sender: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    for (index, command_line) in commands.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let started_at = Instant::now();
        let _ = sender.send(Event::Started { index });

        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(terminal_size.as_pty_size()) {
            Ok(pair) => pair,
            Err(error) => {
                send_spawn_failure(&sender, index, started_at, error.to_string());
                continue;
            }
        };
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                send_spawn_failure(&sender, index, started_at, error.to_string());
                continue;
            }
        };

        let mut command = shell_command(&command_line);
        configure_color(&mut command, color);
        let mut child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(error) => {
                send_spawn_failure(&sender, index, started_at, error.to_string());
                continue;
            }
        };
        drop(pair.slave);

        // Children see a real terminal, which lets progress bars and color detection work. collapse
        // does not forward interactive input, so close the writable side after spawning.
        if let Ok(writer) = pair.master.take_writer() {
            drop(writer);
        }
        let output_reader = pipe_reader(reader, index, sender.clone());

        let (status, was_cancelled) = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break (child.wait().ok(), true);
            }

            match child.try_wait() {
                Ok(Some(status)) => break (Some(status), false),
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break (child.wait().ok(), false),
            }
        };

        output_reader.finish();

        let result = if was_cancelled {
            CommandResult::Cancelled
        } else if let Some(status) = status {
            CommandResult::Exited {
                code: status.exit_code(),
                signal: status.signal().map(str::to_owned),
            }
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

fn send_spawn_failure(sender: &Sender<Event>, index: usize, started_at: Instant, error: String) {
    let _ = sender.send(Event::Finished {
        index,
        result: CommandResult::SpawnFailed(error),
        elapsed: started_at.elapsed(),
    });
}

struct PipeReader {
    handle: thread::JoinHandle<()>,
    done: Receiver<()>,
}

impl PipeReader {
    fn finish(self) {
        // Some commands start daemons that accidentally retain the terminal. Waiting forever for
        // EOF would prevent the next command from starting, even though the command itself exited.
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
fn shell_command(command_line: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(command_line);
    command
}

#[cfg(windows)]
fn shell_command(command_line: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("cmd");
    command.arg("/C");
    command.arg(command_line);
    command
}

fn configure_color(command: &mut CommandBuilder, color: ColorMode) {
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

    const TEST_TERMINAL: TerminalSize = TerminalSize {
        rows: 24,
        cols: 100,
    };

    #[test]
    fn runs_commands_in_order_and_captures_output() {
        let (sender, receiver) = mpsc::channel();
        run(
            vec!["printf first".into(), "printf second".into()],
            ColorMode::Auto,
            TEST_TERMINAL,
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

        let mut terminal = vt100::Parser::new(24, 100, 100);
        terminal.process(&output);
        let rendered = terminal.screen().contents();

        assert_eq!(starts, [0, 1]);
        assert!(rendered.contains("first"));
        assert!(rendered.contains("second"));
        assert!(matches!(events.last(), Some(Event::AllFinished { .. })));
    }

    #[test]
    fn child_detects_a_terminal() {
        let (sender, receiver) = mpsc::channel();
        run(
            vec!["if [ -t 2 ]; then printf tty; fi".into()],
            ColorMode::Auto,
            TEST_TERMINAL,
            sender,
            Arc::new(AtomicBool::new(false)),
        );
        let output = receiver
            .into_iter()
            .filter_map(|event| match event {
                Event::Output { bytes, .. } => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();

        assert!(String::from_utf8_lossy(&output).contains("tty"));
    }

    #[cfg(unix)]
    #[test]
    fn advances_when_a_background_process_retains_the_terminal() {
        let (sender, receiver) = mpsc::channel();
        let started_at = Instant::now();
        let worker = thread::spawn(move || {
            run(
                vec!["sleep 3 & printf first".into(), "printf second".into()],
                ColorMode::Auto,
                TEST_TERMINAL,
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
            "the retained terminal blocked the next command from starting"
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
                TEST_TERMINAL,
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
