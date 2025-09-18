use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, PtyPair, PtySize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task;

#[derive(Clone, Debug)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        }
    }

    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }
}

#[derive(Clone, Debug)]
pub struct SpawnConfig {
    pub command: Command,
    pub cols: u16,
    pub rows: u16,
}

impl SpawnConfig {
    pub fn new(command: Command, cols: u16, rows: u16) -> Self {
        Self {
            command,
            cols,
            rows,
        }
    }
}

pub struct PtyProcess {
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
}

impl PtyProcess {
    pub fn spawn(config: SpawnConfig) -> Result<(Self, PtyReader, PtyWriter)> {
        let mut pair = spawn_pair(config.rows, config.cols)?;
        let child = spawn_child(&mut pair, &config.command)?;
        let master = pair.master;
        let reader = master.try_clone_reader().context("clone PTY reader")?;
        let writer = master.take_writer().context("take PTY writer")?;

        let process = Self {
            master: Arc::new(Mutex::new(master)),
            child: Arc::new(Mutex::new(Some(child))),
        };

        Ok((process, PtyReader::new(reader), PtyWriter::new(writer)))
    }

    pub async fn wait(&self) -> Result<()> {
        let child = self.child.clone();
        task::spawn_blocking(move || {
            let mut guard = child.lock().unwrap();
            if let Some(child) = guard.as_mut() {
                child.wait().context("wait for PTY child")?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("join child wait task")??;
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self.master.lock().unwrap();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        master.resize(size).context("resize PTY")
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
pub struct PtyReader {
    reader: Arc<Mutex<Box<dyn std::io::Read + Send>>>,
}

impl PtyReader {
    const CHUNK: usize = 4096;

    fn new(reader: Box<dyn std::io::Read + Send>) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
        }
    }

    pub async fn read_chunk(&self) -> Result<Option<Vec<u8>>> {
        let reader = self.reader.clone();
        task::spawn_blocking(move || loop {
            let mut guard = reader.lock().unwrap();
            let mut buffer = vec![0u8; Self::CHUNK];
            match guard.read(&mut buffer) {
                Ok(0) => return Ok(None),
                Ok(n) => {
                    buffer.truncate(n);
                    return Ok(Some(buffer));
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            }
        })
        .await
        .context("join PTY read task")?
    }
}

#[derive(Clone)]
pub struct PtyWriter {
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
}

impl PtyWriter {
    fn new(writer: Box<dyn std::io::Write + Send>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut guard = self.writer.lock().unwrap();
        guard.write_all(bytes).context("write to PTY")?;
        guard.flush().context("flush PTY writer")?;
        Ok(())
    }

    pub fn write_str(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes())
    }
}

fn spawn_pair(rows: u16, cols: u16) -> Result<PtyPair> {
    let pty_system = native_pty_system();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    pty_system.openpty(size).context("open PTY pair")
}

fn spawn_child(pair: &mut PtyPair, command: &Command) -> Result<Box<dyn Child + Send + Sync>> {
    let mut cmd = CommandBuilder::new(&command.program);
    for arg in &command.args {
        cmd.arg(arg);
    }
    for (key, value) in &command.env {
        cmd.env(key, value);
    }
    if let Some(cwd) = &command.cwd {
        cmd.cwd(Path::new(cwd));
    }
    pair.slave.spawn_command(cmd).context("spawn PTY child")
}

pub fn resize_pty(process: &PtyProcess, cols: u16, rows: u16) -> Result<()> {
    process.resize(cols, rows)
}

use std::io::Read;
use std::io::Write;
