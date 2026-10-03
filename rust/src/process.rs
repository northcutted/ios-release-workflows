use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Arg {
    Text(String),
    Secret { secret_env: String },
}
impl From<String> for Arg {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}
impl From<&str> for Arg {
    fn from(v: &str) -> Self {
        Self::Text(v.to_owned())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub program: String,
    pub args: Vec<Arg>,
    pub env: BTreeMap<String, String>,
    pub timeout_seconds: u64,
}

impl Step {
    pub fn new(
        program: &str,
        args: impl IntoIterator<Item = impl Into<String>>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(|v| Arg::Text(v.into())).collect(),
            env: BTreeMap::new(),
            timeout_seconds,
        }
    }
    pub fn developer(mut self, path: &str) -> Self {
        self.env.insert("DEVELOPER_DIR".into(), path.into());
        self
    }
}

#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
}

pub trait Executor {
    fn run(&mut self, step: &Step, root: &Path, log: Option<&Path>) -> Result<Output>;
}

// Every exit path, including log-write errors, terminates the owned process group.
struct OwnedChild {
    child: Child,
    armed: bool,
}
impl OwnedChild {
    fn kill(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.armed {
            self.kill();
        }
    }
}

fn reader(
    mut pipe: impl Read + Send + 'static,
    tx: mpsc::SyncSender<(bool, Vec<u8>)>,
    stdout: bool,
) -> thread::JoinHandle<std::io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let count = pipe.read(&mut buffer)?;
            if count == 0 || tx.send((stdout, buffer[..count].to_vec())).is_err() {
                return Ok(());
            }
        }
    })
}

pub struct Native;
impl Executor for Native {
    fn run(&mut self, step: &Step, root: &Path, log: Option<&Path>) -> Result<Output> {
        ensure!(step.timeout_seconds > 0, "A process timeout is required");
        let mut logfile = log
            .map(|path| -> Result<File> {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut options = fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                Ok(options.open(path)?)
            })
            .transpose()?;
        let mut command = Command::new(&step.program);
        for arg in &step.args {
            match arg {
                Arg::Text(value) => {
                    command.arg(value);
                }
                Arg::Secret { secret_env } => {
                    command.arg(std::env::var(secret_env).with_context(|| {
                        format!("Missing secret environment variable {secret_env}")
                    })?);
                }
            }
        }
        command
            .envs(&step.env)
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .with_context(|| format!("Cannot launch {}", step.program))?;
        let mut owned = OwnedChild { child, armed: true };
        let stdout = owned
            .child
            .stdout
            .take()
            .context("Missing process output")?;
        let stderr = owned
            .child
            .stderr
            .take()
            .context("Missing process diagnostics")?;
        // Bounded chunks avoid unbounded lines and producer queues during xcodebuild.
        let (tx, rx) = mpsc::sync_channel(32);
        let out_thread = reader(stdout, tx.clone(), true);
        let err_thread = reader(stderr, tx, false);
        let started = Instant::now();
        let mut captured = Vec::new();
        let mut status = None;
        let mut timed_out = false;
        loop {
            let mut disconnected = false;
            for _ in 0..64 {
                match rx.try_recv() {
                    Ok((is_stdout, chunk)) => {
                        if let Some(file) = &mut logfile {
                            file.write_all(&chunk)?;
                            std::io::stderr().write_all(&chunk)?;
                        } else if is_stdout {
                            ensure!(
                                captured.len() + chunk.len() <= 32 * 1024 * 1024,
                                "{} output exceeded the 32 MiB inspection limit",
                                step.program
                            );
                            captured.extend_from_slice(&chunk);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if status.is_none() {
                status = owned
                    .child
                    .try_wait()?
                    .map(|value| value.code().unwrap_or(1));
            }
            if status.is_some() && disconnected {
                break;
            }
            // The deadline covers descendants holding pipes open after the parent exits.
            if started.elapsed() >= Duration::from_secs(step.timeout_seconds) && !timed_out {
                owned.kill();
                status = Some(124);
                timed_out = true;
            }
            if timed_out
                && started.elapsed()
                    >= Duration::from_secs(step.timeout_seconds) + Duration::from_secs(1)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        owned.armed = false;
        if let Some(file) = &mut logfile {
            file.sync_all()?;
        }
        for handle in [out_thread, err_thread] {
            if handle.is_finished() {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("Process output reader failed"))??;
            }
        }
        Ok(Output {
            status: status.unwrap_or(1),
            stdout: String::from_utf8(captured).context("Tool output is not UTF-8")?,
        })
    }
}

pub fn checked(executor: &mut impl Executor, step: &Step, root: &Path) -> Result<String> {
    let result = executor.run(step, root, None)?;
    if result.status != 0 {
        bail!(
            "{} exited {}; retained diagnostics may explain the failure",
            step.program,
            result.status
        );
    }
    Ok(result.stdout.trim().to_owned())
}
