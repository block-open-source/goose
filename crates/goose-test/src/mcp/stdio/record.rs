use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone)]
enum StreamType {
    Stdin,
    Stdout,
    Stderr,
}

fn handle_output_stream<R: BufRead + Send + 'static>(
    reader: R,
    sender: mpsc::Sender<(StreamType, String)>,
    stream_type: StreamType,
    mut output_writer: Box<dyn Write + Send>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let _ = sender.send((stream_type.clone(), line.clone()));

                    if writeln!(output_writer, "{}", line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn handle_stdin_stream(
    mut child_stdin: ChildStdin,
    sender: mpsc::Sender<(StreamType, String)>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let stdin = io::stdin();

        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    let _ = sender.send((StreamType::Stdin, line.clone()));

                    if writeln!(child_stdin, "{}", line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

pub fn record(log_file_path: &String, cmd: &String, cmd_args: &[String]) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_file_path)?;

    let mut child = Command::new(cmd)
        .args(cmd_args.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .inspect_err(|e| eprintln!("Failed to execute command '{}': {}", cmd, e))?;

    let child_stdin = child.stdin.take().unwrap();
    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    let stdin_handle = handle_stdin_stream(child_stdin, tx.clone());
    let stdout_handle = handle_output_stream(
        BufReader::new(child_stdout),
        tx.clone(),
        StreamType::Stdout,
        Box::new(io::stdout()),
    );
    let stderr_handle = handle_output_stream(
        BufReader::new(child_stderr),
        tx.clone(),
        StreamType::Stderr,
        Box::new(io::stderr()),
    );

    thread::spawn(move || {
        let mut log_file = log_file;
        for (stream_type, line) in rx {
            let prefix = match stream_type {
                StreamType::Stdin => "STDIN",
                StreamType::Stdout => "STDOUT",
                StreamType::Stderr => "STDERR",
            };
            if let Err(e) = writeln!(log_file, "{}: {}", prefix, line) {
                eprintln!("Error writing to log file: {}", e);
            }
            log_file.flush().ok();
        }
    });

    child.wait()?;

    stdin_handle.join().ok();
    stdout_handle.join().ok();
    stderr_handle.join().ok();

    Ok(())
}
