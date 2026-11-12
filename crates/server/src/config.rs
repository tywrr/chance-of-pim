//! `config.toml`: accounts and policy. Secrets never live in the file; each account names the
//! environment variable that holds its password.

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: Server,
    pub mail: Mail,
    pub contacts: Dav,
    pub calendars: Dav,
    pub guide: Guide,
}

/// The owner's own notes for Claude, added to the connector's instructions: what each calendar
/// or folder is for, conventions to follow.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Guide {
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Server {
    pub listen: String,
    pub default_timezone: String,
    pub allowed_origins: Vec<String>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            default_timezone: "UTC".into(),
            allowed_origins: vec!["https://claude.ai".into(), "https://claude.com".into()],
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Mail {
    pub accounts: Vec<MailAccount>,
    pub policy: MailPolicy,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct MailPolicy {
    /// Addresses (`a@b.c`) or domains (`@b.c`) mail may be sent to; empty means anywhere.
    pub send_to: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MailAccount {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub default: bool,
    /// Identities this account may send as, `Name <address>`; the first is the default.
    pub from: Vec<String>,
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub trash_mailbox: Option<String>,
    pub drafts_mailbox: Option<String>,
    pub sent_mailbox: Option<String>,
    /// Save a copy to Sent after sending (most servers don't do it themselves).
    #[serde(default = "yes")]
    pub append_to_sent: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password_env: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Dav {
    pub accounts: Vec<DavAccount>,
}

#[derive(Debug, Deserialize)]
pub struct DavAccount {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub default: bool,
    pub url: String,
    pub user: String,
    pub password_env: String,
    /// Collection names or slugs that may be read but never written.
    #[serde(default)]
    pub protected: Vec<String>,
}

pub fn password(env: &str) -> Result<String, String> {
    match std::env::var(env) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim_end_matches(['\r', '\n']).to_owned()),
        _ => Err(format!("its password ({env}) is not set on the server")),
    }
}

impl Endpoint {
    pub fn server(&self) -> Result<chance_of_pim_mail::Server, String> {
        Ok(chance_of_pim_mail::Server {
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            password: password(&self.password_env)?,
        })
    }
}

impl DavAccount {
    pub fn client(&self) -> Result<chance_of_pim_dav::Client, String> {
        chance_of_pim_dav::Client::new(chance_of_pim_dav::Account {
            url: self.url.clone(),
            user: self.user.clone(),
            password: password(&self.password_env)?,
        })
        .map_err(|e| e.to_string())
    }
}

pub fn load(path: &str) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let config: Config = toml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut seen = std::collections::HashSet::new();
    for id in config.mail.accounts.iter().map(|a| &a.id) {
        if id.contains(':') || !seen.insert(format!("mail/{id}")) {
            return Err(format!(
                "mail account id {id:?} is repeated or contains ':'"
            ));
        }
    }
    for (kind, list) in [
        ("contacts", &config.contacts),
        ("calendars", &config.calendars),
    ] {
        for id in list.accounts.iter().map(|a| &a.id) {
            if id.contains(':') || !seen.insert(format!("{kind}/{id}")) {
                return Err(format!(
                    "{kind} account id {id:?} is repeated or contains ':'"
                ));
            }
        }
    }
    for a in &config.mail.accounts {
        if a.from.is_empty() {
            return Err(format!(
                "mail account {:?} needs at least one `from` identity",
                a.id
            ));
        }
    }
    Ok(config)
}
