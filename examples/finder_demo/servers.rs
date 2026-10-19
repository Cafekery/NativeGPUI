use std::path::PathBuf;

use native_gpui::tokens::SystemColor;

use crate::prefs;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    Smb,
    Sftp,
    WebDav,
    Afp,
    Nfs,
    Ftp,
}

impl Protocol {
    pub(crate) const ALL: [Protocol; 6] = [
        Protocol::Smb,
        Protocol::Sftp,
        Protocol::WebDav,
        Protocol::Afp,
        Protocol::Nfs,
        Protocol::Ftp,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Protocol::Smb => "SMB",
            Protocol::Sftp => "SFTP over SSH",
            Protocol::WebDav => "WebDAV",
            Protocol::Afp => "AFP",
            Protocol::Nfs => "NFS",
            Protocol::Ftp => "FTP",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Protocol::Smb => {
                "Windows and Mac file sharing. The usual choice for a NAS or another computer on your network."
            }
            Protocol::Sftp => {
                "Browse any computer you can reach with ssh. Files you open are copied to a local cache first."
            }
            Protocol::WebDav => "Web servers, Nextcloud and other cloud drives that speak WebDAV.",
            Protocol::Afp => {
                "Apple\u{2019}s older file sharing, for Macs on old systems and Time Capsules."
            }
            Protocol::Nfs => "Unix file sharing, usually an export from a Linux server or NAS.",
            Protocol::Ftp => "Plain, unencrypted FTP. macOS mounts it read-only.",
        }
    }

    pub(crate) fn default_port(self) -> u16 {
        match self {
            Protocol::Smb => 445,
            Protocol::Sftp => 22,
            Protocol::WebDav => 443,
            Protocol::Afp => 548,
            Protocol::Nfs => 2049,
            Protocol::Ftp => 21,
        }
    }

    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Protocol::Sftp => "terminal",
            Protocol::WebDav | Protocol::Ftp => "globe",
            _ => "network",
        }
    }

    pub(crate) fn tint(self) -> SystemColor {
        match self {
            Protocol::Smb => SystemColor::SystemBlue,
            Protocol::Sftp => SystemColor::SystemGray,
            Protocol::WebDav => SystemColor::SystemTeal,
            Protocol::Afp => SystemColor::SystemPurple,
            Protocol::Nfs => SystemColor::SystemOrange,
            Protocol::Ftp => SystemColor::SystemGreen,
        }
    }

    pub(crate) fn shares(self) -> bool {
        matches!(self, Protocol::Smb | Protocol::Afp)
    }

    fn from_scheme(scheme: &str) -> Option<Protocol> {
        Some(match scheme.to_ascii_lowercase().as_str() {
            "smb" | "cifs" => Protocol::Smb,
            "sftp" | "ssh" => Protocol::Sftp,
            "http" | "https" => Protocol::WebDav,
            "afp" => Protocol::Afp,
            "nfs" => Protocol::Nfs,
            "ftp" => Protocol::Ftp,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Saved {
    pub(crate) name: String,
    pub(crate) protocol: Protocol,
    pub(crate) https: bool,
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) port: String,
    pub(crate) path: String,
    pub(crate) identity: String,
}

impl Saved {
    pub(crate) fn new(protocol: Protocol) -> Saved {
        Saved {
            name: String::new(),
            protocol,
            https: true,
            host: String::new(),
            user: String::new(),
            port: String::new(),
            path: String::new(),
            identity: String::new(),
        }
    }

    pub(crate) fn parse(address: &str) -> Option<Saved> {
        let (scheme, rest) = address.trim().split_once("://")?;
        let protocol = Protocol::from_scheme(scheme)?;
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let (user, host_port) = authority
            .rsplit_once('@')
            .map_or(("", authority), |(user, host)| (user, host));
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => (host, port),
            _ => (host_port, ""),
        };
        let mut saved = Saved::new(protocol);
        saved.https = !scheme.eq_ignore_ascii_case("http");
        saved.host = host.to_owned();
        saved.user = user.to_owned();
        saved.port = port.to_owned();
        saved.path = path.trim_start_matches('/').to_owned();
        (!host.is_empty()).then_some(saved)
    }

    pub(crate) fn address(&self) -> String {
        let scheme = match self.protocol {
            Protocol::Smb => "smb",
            Protocol::Sftp => "sftp",
            Protocol::WebDav if self.https => "https",
            Protocol::WebDav => "http",
            Protocol::Afp => "afp",
            Protocol::Nfs => "nfs",
            Protocol::Ftp => "ftp",
        };
        let user = self.user.trim();
        let user = if user.is_empty() {
            String::new()
        } else {
            format!("{user}@")
        };
        let port = self.port().map_or(String::new(), |port| format!(":{port}"));
        let path = self.path.trim().trim_start_matches('/');
        let path = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        format!("{scheme}://{user}{}{port}{path}", self.host.trim())
    }

    pub(crate) fn port(&self) -> Option<u16> {
        let port = self.port.trim().parse::<u16>().ok()?;
        (port != self.protocol.default_port()).then_some(port)
    }

    pub(crate) fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            self.host.trim()
        } else {
            self.name.trim()
        }
    }

    fn record(&self) -> String {
        format!(
            "{}\t{}\t{}",
            self.address(),
            self.name.trim(),
            self.identity.trim()
        )
    }

    fn from_record(record: &str) -> Option<Saved> {
        let mut parts = record.splitn(3, '\t');
        let mut saved = Saved::parse(parts.next()?)?;
        saved.name = parts.next().unwrap_or_default().to_owned();
        saved.identity = parts.next().unwrap_or_default().to_owned();
        Some(saved)
    }
}

pub(crate) fn saved() -> Vec<Saved> {
    (prefs::saved_servers().iter())
        .filter_map(|record| Saved::from_record(record))
        .collect()
}

pub(crate) fn store(saved: &[Saved]) {
    let records: Vec<String> = saved.iter().map(Saved::record).collect();
    prefs::set_saved_servers(&records);
}

pub(crate) fn name_for(root: &str) -> Option<String> {
    let wanted = Saved::parse(root)?.host;
    saved()
        .into_iter()
        .find(|saved| {
            finder_fs::servers::same_host(&saved.host, &wanted) && !saved.name.trim().is_empty()
        })
        .map(|saved| saved.name.trim().to_owned())
}

pub(crate) fn identity_for(address: &str) -> Option<PathBuf> {
    let wanted = Saved::parse(address)?.address();
    saved()
        .into_iter()
        .find(|saved| saved.address() == wanted && !saved.identity.trim().is_empty())
        .map(|saved| PathBuf::from(saved.identity.trim()))
}
