//! The hand-off to the app's `SSH_ASKPASS` helper.

/// Set on ssh for the helper: the host the prompt is for.
pub const HOST: &str = "FOUNDER_SFTP_HOST";
/// Set on ssh for the helper: the account the prompt is for.
pub const USER: &str = "FOUNDER_SFTP_USER";
/// Set on ssh for the helper: a per-session file it appends one byte to per prompt; empty means attempt 1.
pub const ATTEMPT: &str = "FOUNDER_SFTP_ATTEMPT";
/// Set on ssh for the helper: a private file shared by the server's sessions; on attempt 1 the helper answers from it if non-empty, else from the Keychain; on later attempts it asks the user and writes the answer here.
pub const CACHE: &str = "FOUNDER_SFTP_CACHE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Password,
    /// ssh's question about an unknown or changed host key, fingerprint included.
    HostKey {
        text: String,
    },
    Other(String),
}

/// What ssh is asking for, from the prompt it passes the helper.
pub fn classify(prompt: &str) -> Prompt {
    let lower = prompt.to_lowercase();
    if lower.contains("continue connecting") {
        Prompt::HostKey {
            text: prompt.trim().to_owned(),
        }
    } else if lower.contains("password:") || lower.contains("password for") {
        Prompt::Password
    } else {
        Prompt::Other(prompt.trim().to_owned())
    }
}
