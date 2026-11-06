//! `paddock device-serve`: the supervisor of a device program
//! (`paddock::devices::supervise`). Hidden: the broker starts it.

use anyhow::Result;
use clap::Args;
use paddock::devices::config::Shape;
use paddock::devices::supervise::{self, Options};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct DeviceServeArgs {
    /// The Unix socket to serve HTTP/1.1 on. Its folder must exist.
    #[arg(long)]
    pub(crate) socket: PathBuf,
    /// exec: the command runs once per request. serve: it runs once and
    /// listens on $PADDOCK_DEVICE_SOCKET.
    #[arg(long, value_parser = ["exec", "serve"])]
    pub(crate) shape: String,
    /// Seconds per request.
    #[arg(long, default_value_t = paddock::devices::config::DEFAULT_TIMEOUT)]
    pub(crate) timeout: u64,
    /// The largest request body, in bytes.
    #[arg(long, default_value_t = paddock::devices::config::DEFAULT_MAX_BODY)]
    pub(crate) max_request: usize,
    /// The largest response body, in bytes.
    #[arg(long, default_value_t = paddock::devices::config::DEFAULT_MAX_BODY)]
    pub(crate) max_response: usize,
    /// Requests at work at the same time (default: 1 for exec, 4 for serve).
    #[arg(long)]
    pub(crate) concurrency: Option<u32>,
    /// exec: the response is the raw standard output as a chunked body,
    /// with the exit status in the trailer Paddock-Exit.
    #[arg(long)]
    pub(crate) stream: bool,
    /// serve: seconds to wait for the program's /v1/health.
    #[arg(long, default_value_t = paddock::devices::config::DEFAULT_READY)]
    pub(crate) ready: u64,
    /// PORT:SOCKET: listen on 127.0.0.1:PORT and copy each connection to
    /// the socket (repeatable).
    #[arg(long, value_name = "PORT:SOCKET")]
    pub(crate) connect: Vec<String>,
    /// The log file (default: stderr). Capped at 10 MiB, rotated once.
    #[arg(long)]
    pub(crate) log: Option<PathBuf>,
    /// A variable of this environment that the program gets on top of the
    /// base set (repeatable).
    #[arg(long, value_name = "NAME")]
    pub(crate) env_pass: Vec<String>,
    /// The program runs on the host as the user: it gets this whole
    /// environment, not the base set.
    #[arg(long)]
    pub(crate) host: bool,
    /// The working folder of the program (default: a fresh empty folder
    /// per exec request).
    #[arg(long)]
    pub(crate) cwd: Option<PathBuf>,
    /// The program and its fixed arguments, after `--`.
    #[arg(last = true, allow_hyphen_values = true, required = true)]
    pub(crate) command: Vec<String>,
}

impl DeviceServeArgs {
    pub(crate) fn options(&self) -> Result<Options> {
        let shape = if self.shape == "serve" {
            Shape::Serve
        } else {
            Shape::Exec
        };
        let mut o = Options::new(&self.socket, shape, self.command.clone());
        o.timeout = self.timeout.clamp(1, paddock::devices::config::MAX_TIMEOUT);
        o.max_request = self
            .max_request
            .clamp(1, paddock::devices::config::MAX_BODY_CAP);
        o.max_response = self
            .max_response
            .clamp(1, paddock::devices::config::MAX_BODY_CAP);
        if let Some(c) = self.concurrency {
            o.concurrency = c.clamp(1, paddock::devices::config::MAX_CONCURRENCY);
        }
        o.stream = self.stream;
        o.ready = self.ready.clamp(1, paddock::devices::config::MAX_READY);
        o.connect = self
            .connect
            .iter()
            .map(|c| supervise::parse_connect(c))
            .collect::<Result<Vec<_>>>()?;
        o.log = self.log.clone();
        o.env_pass = self.env_pass.clone();
        o.host = self.host;
        o.cwd = self.cwd.clone();
        Ok(o)
    }
}

pub(crate) fn run(args: &DeviceServeArgs) -> Result<i32> {
    supervise::main(args.options()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        a: DeviceServeArgs,
    }

    #[test]
    fn the_command_line_becomes_options() {
        let c = Cli::parse_from([
            "device-serve",
            "--socket",
            "/tmp/x/dev.sock",
            "--shape",
            "exec",
            "--timeout",
            "5",
            "--stream",
            "--connect",
            "2222:/tmp/x/0.sock",
            "--",
            "sh",
            "-c",
            "cat",
        ]);
        let o = c.a.options().unwrap();
        assert_eq!(o.shape, Shape::Exec);
        assert_eq!(o.timeout, 5);
        assert!(o.stream);
        assert_eq!(o.concurrency, 1);
        assert_eq!(o.connect, [(2222, PathBuf::from("/tmp/x/0.sock"))]);
        assert_eq!(o.command, ["sh", "-c", "cat"]);
        assert!(!o.host);
        assert_eq!(o.cwd, None);
        let c = Cli::parse_from([
            "device-serve",
            "--socket",
            "/tmp/x/dev.sock",
            "--shape",
            "serve",
            "--host",
            "--cwd",
            "/home/demo/notes",
            "--",
            "./upper.py",
        ]);
        let o = c.a.options().unwrap();
        assert_eq!(o.shape, Shape::Serve);
        assert_eq!(o.concurrency, 4);
        assert_eq!(o.ready, 10);
        assert!(o.host);
        assert_eq!(o.cwd, Some(PathBuf::from("/home/demo/notes")));
        assert!(supervise::parse_connect("x:/s").is_err());
        assert!(supervise::parse_connect("22:").is_err());
    }
}
