use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardBackend {
    MacOs,
    Wayland,
    X11,
}

#[derive(Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    args: &'static [&'static str],
}

impl ClipboardBackend {
    fn write_command(self) -> CommandSpec {
        match self {
            Self::MacOs => CommandSpec {
                name: "pbcopy",
                args: &[],
            },
            Self::Wayland => CommandSpec {
                name: "wl-copy",
                args: &[],
            },
            Self::X11 => CommandSpec {
                name: "xclip",
                args: &["-selection", "clipboard", "-in"],
            },
        }
    }

    fn read_command(self) -> CommandSpec {
        match self {
            Self::MacOs => CommandSpec {
                name: "pbpaste",
                args: &[],
            },
            Self::Wayland => CommandSpec {
                name: "wl-paste",
                args: &["--no-newline"],
            },
            Self::X11 => CommandSpec {
                name: "xclip",
                args: &["-selection", "clipboard", "-out"],
            },
        }
    }
}

fn select_backend(
    platform: &str,
    wayland_display: Option<&OsStr>,
) -> Result<ClipboardBackend, String> {
    match platform {
        "macos" => Ok(ClipboardBackend::MacOs),
        "linux" if wayland_display.is_some_and(|value| !value.is_empty()) => {
            Ok(ClipboardBackend::Wayland)
        }
        "linux" => Ok(ClipboardBackend::X11),
        other => Err(format!("system clipboard is not supported on {other}")),
    }
}

fn current_backend() -> Result<ClipboardBackend, String> {
    select_backend(
        std::env::consts::OS,
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )
}

struct CommandOutput {
    success: bool,
    status: String,
    stdout: Vec<u8>,
}

trait CommandRunner {
    fn run(&self, command: CommandSpec, input: Option<&[u8]>) -> Result<CommandOutput, String>;
}

struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&self, command: CommandSpec, input: Option<&[u8]>) -> Result<CommandOutput, String> {
        let mut process = Command::new(command.name);
        process.args(command.args);
        if input.is_some() {
            // Do not pipe output from writers: wl-copy forks a serving process,
            // which inherits pipes and would keep wait_with_output blocked.
            process
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        } else {
            process.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", command.name))?;

        let write_error = if let Some(input) = input {
            match child.stdin.take() {
                Some(mut stdin) => stdin
                    .write_all(input)
                    .err()
                    .map(|error| format!("failed to write to {}: {error}", command.name)),
                None => Some(format!("failed to open {} stdin", command.name)),
            }
        } else {
            None
        };

        // Always reap a successfully spawned process, including after a broken
        // stdin pipe, so repeated clipboard failures cannot accumulate zombies.
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to wait for {}: {error}", command.name))?;
        if let Some(error) = write_error {
            return Err(error);
        }
        Ok(CommandOutput {
            success: output.status.success(),
            status: output.status.to_string(),
            stdout: output.stdout,
        })
    }
}

fn write_with(
    backend: ClipboardBackend,
    runner: &impl CommandRunner,
    text: &str,
) -> Result<(), String> {
    let command = backend.write_command();
    let output = runner.run(command, Some(text.as_bytes()))?;
    if output.success {
        Ok(())
    } else {
        Err(format!("{} exited with {}", command.name, output.status))
    }
}

fn read_with(backend: ClipboardBackend, runner: &impl CommandRunner) -> Result<String, String> {
    let output = runner.run(backend.read_command(), None)?;
    if !output.success {
        if backend != ClipboardBackend::MacOs {
            // wl-paste and xclip report an empty or unowned clipboard with a
            // non-zero exit code. Reading an empty clipboard is not actionable.
            return Ok(String::new());
        }
        return Err(format!(
            "{} exited with {}",
            backend.read_command().name,
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("clipboard did not contain UTF-8 text: {error}"))
}

pub(crate) fn write(text: &str) -> Result<(), String> {
    write_with(current_backend()?, &ProcessCommandRunner, text)
}

pub(crate) fn read() -> Result<String, String> {
    read_with(current_backend()?, &ProcessCommandRunner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeRunner {
        calls: RefCell<Vec<(String, Vec<String>, Option<Vec<u8>>)>>,
        output: RefCell<Option<Result<CommandOutput, String>>>,
    }

    impl FakeRunner {
        fn returning(output: Result<CommandOutput, String>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                output: RefCell::new(Some(output)),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, command: CommandSpec, input: Option<&[u8]>) -> Result<CommandOutput, String> {
            self.calls.borrow_mut().push((
                command.name.to_string(),
                command.args.iter().map(|arg| (*arg).to_string()).collect(),
                input.map(<[u8]>::to_vec),
            ));
            self.output.borrow_mut().take().expect("unexpected command")
        }
    }

    fn successful_output(stdout: &[u8]) -> CommandOutput {
        CommandOutput {
            success: true,
            status: "exit status: 0".to_string(),
            stdout: stdout.to_vec(),
        }
    }

    #[test]
    fn backend_selection_uses_wayland_only_for_a_nonempty_display() {
        assert_eq!(
            select_backend("linux", Some(OsStr::new("wayland-1"))),
            Ok(ClipboardBackend::Wayland)
        );
        assert_eq!(
            select_backend("linux", Some(OsStr::new(""))),
            Ok(ClipboardBackend::X11)
        );
        assert_eq!(select_backend("linux", None), Ok(ClipboardBackend::X11));
        assert_eq!(select_backend("macos", None), Ok(ClipboardBackend::MacOs));
    }

    #[test]
    fn backend_commands_are_injected_into_the_runner() {
        let runner = FakeRunner::returning(Ok(successful_output(b"text")));
        assert_eq!(
            read_with(ClipboardBackend::Wayland, &runner),
            Ok("text".to_string())
        );
        assert_eq!(
            runner.calls.into_inner(),
            vec![(
                "wl-paste".to_string(),
                vec!["--no-newline".to_string()],
                None
            )]
        );
    }

    #[test]
    fn unsuccessful_read_is_an_empty_clipboard() {
        let runner = FakeRunner::returning(Ok(CommandOutput {
            success: false,
            status: "exit status: 1".to_string(),
            stdout: Vec::new(),
        }));
        assert_eq!(
            read_with(ClipboardBackend::Wayland, &runner),
            Ok(String::new())
        );
    }

    #[test]
    fn missing_command_error_names_the_selected_command() {
        let runner = FakeRunner::returning(Err(
            "failed to start wl-copy: No such file or directory".to_string(),
        ));
        let error = write_with(ClipboardBackend::Wayland, &runner, "text").unwrap_err();
        assert!(error.contains("wl-copy"), "{error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a running Wayland session and wl-clipboard"]
    fn real_wayland_clipboard_round_trip() {
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_some());
        let text = format!("eseqlisp clipboard test {}", std::process::id());
        write_with(ClipboardBackend::Wayland, &ProcessCommandRunner, &text).unwrap();
        assert_eq!(
            read_with(ClipboardBackend::Wayland, &ProcessCommandRunner),
            Ok(text)
        );
    }
}
