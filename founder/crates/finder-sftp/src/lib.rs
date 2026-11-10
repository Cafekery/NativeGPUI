//! SFTP version 3 spoken over the system's OpenSSH, pipelined and spread across several connections.

pub mod askpass;
pub mod keychain;
mod server;
mod session;
mod transfer;
mod wire;

#[cfg(test)]
mod tests;

use std::fmt::{self, Write as _};
use std::io;
use std::path::PathBuf;

pub use server::Server;

/// The connection a [`Server`] handle's transfers ride, so lookahead never queues ahead of the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lane {
    /// Metadata calls and mutations.
    Control,
    /// What the user is looking at or asked for; large transfers also borrow the `Bulk` and `Preview` connections.
    #[default]
    Interactive,
    /// Small range reads for thumbnails.
    Preview,
    /// Whole files fetched ahead of need.
    Bulk,
    /// Directory reads ahead of navigation; `list` and `stat` use it too.
    Listing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    /// Empty means the login home.
    pub path: String,
    pub identity: Option<PathBuf>,
}

impl Address {
    /// `sftp://[user@]host[:port][/path]`, or the same without the scheme.
    pub fn parse(text: &str) -> Option<Address> {
        let text = text.trim();
        let rest = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("sftp") => rest,
            Some(_) => return None,
            None => text,
        };
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let (user, host_port) = match authority.rsplit_once('@') {
            Some((user, host_port)) => (Some(user), host_port),
            None => (None, authority),
        };
        let (host, port) = match host_port.strip_prefix('[') {
            Some(bracketed) => {
                let (host, after) = bracketed.split_once(']')?;
                if after.is_empty() {
                    (host, None)
                } else {
                    (host, Some(after.strip_prefix(':')?))
                }
            }
            None => match host_port.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            },
        };
        let port = match port {
            Some(port) => Some(port.parse().ok().filter(|port| *port != 0)?),
            None => None,
        };
        // A leading '-' would reach ssh as an option.
        let bad = |part: &str| {
            part.is_empty() || part.starts_with('-') || part.contains(char::is_whitespace)
        };
        if bad(host) || user.is_some_and(bad) {
            return None;
        }
        Some(Address {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
            path: path.to_owned(),
            identity: None,
        })
    }

    /// The address without its path, e.g. `sftp://user@host:2222`.
    pub fn root(&self) -> String {
        let mut root = String::from("sftp://");
        if let Some(user) = &self.user {
            root.push_str(user);
            root.push('@');
        }
        if self.host.contains(':') {
            write!(root, "[{}]", self.host).unwrap();
        } else {
            root.push_str(&self.host);
        }
        if let Some(port) = self.port {
            write!(root, ":{port}").unwrap();
        }
        root
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub permissions: Option<u32>,
    pub atime: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn kind(&self) -> Kind {
        match self.permissions.map(|mode| mode & 0o170000) {
            Some(0o100000) => Kind::File,
            Some(0o040000) => Kind::Dir,
            Some(0o120000) => Kind::Symlink,
            _ => Kind::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The server's `ls -l` line.
    pub longname: String,
    pub attrs: Attrs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub available: u64,
    pub total: u64,
}

#[derive(Debug)]
pub enum Error {
    Status {
        code: u32,
        message: String,
    },
    /// The connection ended before SFTP started; carries what ssh printed.
    Connect(String),
    Disconnected,
    Io(io::Error),
    Protocol(String),
    Cancelled,
}

impl Error {
    pub fn is_not_found(&self) -> bool {
        self.code() == Some(wire::FX_NO_SUCH_FILE)
    }

    pub fn is_permission(&self) -> bool {
        self.code() == Some(wire::FX_PERMISSION_DENIED)
    }

    fn is_eof(&self) -> bool {
        self.code() == Some(wire::FX_EOF)
    }

    fn code(&self) -> Option<u32> {
        match self {
            Error::Status { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Status { code, message } if message.is_empty() => {
                write!(f, "SFTP status {code}")
            }
            Error::Status { message: text, .. } | Error::Connect(text) | Error::Protocol(text) => {
                f.write_str(text)
            }
            Error::Disconnected => f.write_str("the connection to the server closed"),
            Error::Io(error) => error.fmt(f),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}
