//! The hand-off to the app's `SSH_ASKPASS` helper.

pub const HOST: &str = "FOUNDER_SFTP_HOST";
pub const USER: &str = "FOUNDER_SFTP_USER";
/// A per-session file it appends one byte to per prompt; empty means attempt 1.
pub const ATTEMPT: &str = "FOUNDER_SFTP_ATTEMPT";
/// A private file shared by the server's sessions; on attempt 1 the helper answers from it if non-empty, else from the Keychain; on later attempts it asks the user and writes the answer here.
pub const CACHE: &str = "FOUNDER_SFTP_CACHE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Password,
    HostKey { text: String },
    Other(String),
}

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
