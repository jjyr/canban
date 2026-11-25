//! Account-scoped Device Flow with serialized refresh and local credential storage.
use crate::{
    Client,
    client::{agent, decode},
};
use anyhow::{Context, Result, ensure};
use canban_types::{Clock, system_clock};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
pub const CLIENT_ID: &str = "Iv23li1ImpryrkMbQjKF";
pub const INSTALL_URL: &str = "https://github.com/apps/canban-app/installations/new";
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub installation_url: Option<String>,
    pub state: String,
    pub login: Option<String>,
    pub user_code: Option<String>,
    pub verification_uri: Option<String>,
    pub expires_at: Option<u64>,
    pub error: Option<String>,
}
#[derive(Deserialize)]
struct Credential {
    access_token: SecretString,
    refresh_token: Option<SecretString>,
    expires_at: Option<u64>,
    client_id: String,
    login: String,
}
#[derive(Deserialize)]
struct Device {
    device_code: SecretString,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}
struct Pending {
    device: Device,
    expires: u64,
    next: u64,
}
#[derive(Default)]
struct State {
    loaded: bool,
    credential: Option<Credential>,
    pending: Option<Pending>,
    error: Option<String>,
}
pub struct Service {
    generation: AtomicU64,
    pub cache: std::sync::Arc<crate::cache::Cache>,
    installation_url: Option<String>,
    home: std::path::PathBuf,
    client_id: String,
    state: Mutex<State>,
    clock: Arc<dyn Clock>,
}
#[derive(Deserialize)]
struct Token {
    access_token: Option<SecretString>,
    refresh_token: Option<SecretString>,
    expires_in: Option<u64>,
    error: Option<String>,
}
impl Service {
    pub fn new(home: std::path::PathBuf) -> Self {
        Self::new_with_clock(home, system_clock())
    }
    pub fn new_with_clock(home: std::path::PathBuf, clock: Arc<dyn Clock>) -> Self {
        let client_id = std::env::var("CANBAN_GITHUB_CLIENT_ID").unwrap_or(CLIENT_ID.into());
        let installation_url = std::env::var("CANBAN_GITHUB_INSTALL_URL")
            .ok()
            .filter(|url| {
                url.starts_with("https://github.com/apps/") && url.ends_with("/installations/new")
            })
            .or_else(|| (client_id == CLIENT_ID).then(|| INSTALL_URL.into()));
        Self {
            generation: AtomicU64::new(0),
            cache: crate::cache::Cache::new_with_clock(
                home.join("cache/github/app"),
                clock.clone(),
            ),
            installation_url,
            home,
            client_id,
            state: Mutex::new(State::default()),
            clock,
        }
    }
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }
    fn seconds(&self) -> u64 {
        self.clock.now_ms() / 1000
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    fn load(&self, s: &mut State) -> Result<()> {
        if !s.loaded {
            s.credential = crate::credentials::read(&self.home)?
                .map(|b| serde_json::from_slice(&b))
                .transpose()?;
            s.loaded = true;
        }
        Ok(())
    }
    fn save(&self, c: &Credential) -> Result<()> {
        crate::credentials::store(
            &self.home,
            Some(&serde_json::to_vec(
                &json!({"access_token":c.access_token.expose_secret(),"refresh_token":c.refresh_token.as_ref().map(ExposeSecret::expose_secret),"expires_at":c.expires_at,"client_id":c.client_id,"login":c.login}),
            )?),
        )
    }
    pub fn status(&self) -> Status {
        let mut s = self.state.lock().unwrap();
        if let Err(e) = self.load(&mut s) {
            s.error = Some(format!("Credential store unavailable: {e}"));
        }
        let p = s.pending.as_ref();
        Status {
            installation_url: self.installation_url.clone(),
            state: if p.is_some() {
                "authorizing"
            } else if s.credential.is_some() {
                "connected"
            } else {
                "disconnected"
            }
            .into(),
            login: s.credential.as_ref().map(|c| c.login.clone()),
            user_code: p.map(|p| p.device.user_code.clone()),
            verification_uri: p.map(|p| p.device.verification_uri.clone()),
            expires_at: p.map(|p| p.expires),
            error: s.error.clone(),
        }
    }
    pub fn begin(&self) -> Result<Status> {
        let mut s = self.state.lock().unwrap();
        let device: Device = decode(
            agent()
                .post("https://github.com/login/device/code")
                .header("Accept", "application/json")
                .send_json(json!({"client_id":self.client_id}))?,
        )?;
        ensure!(
            device.verification_uri == "https://github.com/login/device"
                && (1..=900).contains(&device.expires_in)
                && (1..=60).contains(&device.interval),
            "Invalid GitHub device authorization response"
        );
        let expires = self.seconds() + device.expires_in;
        let next = self.seconds() + device.interval;
        self.generation.fetch_add(1, Ordering::SeqCst);
        s.pending = Some(Pending {
            device,
            expires,
            next,
        });
        s.error = None;
        drop(s);
        Ok(self.status())
    }
    pub fn poll(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let Some(p) = s.pending.as_mut() else {
            return Ok(());
        };
        if self.seconds() >= p.expires {
            s.pending = None;
            s.error = Some("GitHub authorization expired. Connect again.".into());
            return Ok(());
        }
        if self.seconds() < p.next {
            return Ok(());
        }
        p.next = self.seconds() + p.device.interval;
        let reply:Token=decode(agent().post("https://github.com/login/oauth/access_token").header("Accept","application/json").send_json(json!({"client_id":self.client_id,"device_code":p.device.device_code.expose_secret(),"grant_type":"urn:ietf:params:oauth:grant-type:device_code"}))?)?;
        match reply.error.as_deref() {
            Some("authorization_pending") => return Ok(()),
            Some("slow_down") => {
                p.device.interval += 5;
                p.next = self.seconds() + p.device.interval;
                return Ok(());
            }
            Some(_) => {
                s.pending = None;
                s.error = Some("GitHub authorization was denied or expired. Connect again.".into());
                return Ok(());
            }
            None => (),
        }
        let access_token = reply.access_token.context("No GitHub access token")?;
        let user: crate::models::User = Client::new(access_token.clone())
            .with_clock(self.clock.clone())
            .get("/user")?;
        let c = Credential {
            access_token,
            refresh_token: reply.refresh_token,
            expires_at: reply.expires_in.map(|n| self.seconds() + n),
            client_id: self.client_id.clone(),
            login: user.login,
        };
        self.save(&c)?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        s.credential = Some(c);
        s.loaded = true;
        s.pending = None;
        s.error = None;
        Ok(())
    }
    pub fn client(&self) -> Result<Client> {
        let mut s = self.state.lock().unwrap();
        self.load(&mut s)?;
        let c = s
            .credential
            .as_ref()
            .context("Connect GitHub on the GitHub page first")?;
        if c.expires_at.is_some_and(|t| t <= self.seconds() + 300) {
            let refresh = c
                .refresh_token
                .as_ref()
                .context("GitHub sign-in expired. Connect again.")?;
            let reply:Token=decode(agent().post("https://github.com/login/oauth/access_token").header("Accept","application/json").send_json(json!({"client_id":c.client_id,"refresh_token":refresh.expose_secret(),"grant_type":"refresh_token"}))?)?;
            ensure!(
                reply.error.is_none(),
                "GitHub authorization expired or was revoked. Connect again."
            );
            let c = Credential {
                access_token: reply
                    .access_token
                    .context("GitHub refresh returned no token")?,
                refresh_token: reply.refresh_token,
                expires_at: reply.expires_in.map(|n| self.seconds() + n),
                client_id: c.client_id.clone(),
                login: c.login.clone(),
            };
            self.save(&c)?;
            s.credential = Some(c);
        }
        let c = s.credential.as_ref().unwrap();
        Ok(Client::new(c.access_token.clone())
            .with_clock(self.clock.clone())
            .with_cache(
                self.cache.clone(),
                format!("github.com:{}:{}", c.client_id, c.login),
            ))
    }
    /// Cache-only projection: never refreshes credentials or performs IO on the network.
    pub fn cached_client(&self) -> Result<Client> {
        let mut s = self.state.lock().unwrap();
        self.load(&mut s)?;
        let c = s.credential.as_ref().context("GitHub is disconnected")?;
        Ok(Client::new(c.access_token.clone())
            .with_clock(self.clock.clone())
            .with_cache(
                self.cache.clone(),
                format!("github.com:{}:{}", c.client_id, c.login),
            ))
    }
    pub fn refresh(&self) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            self.load(&mut state)?;
            let c = state.credential.as_mut().context("Connect GitHub first")?;
            ensure!(
                c.refresh_token.is_some(),
                "This authorization has no refresh token"
            );
            c.expires_at = Some(0);
        }
        self.client()?;
        Ok(())
    }
    pub fn disconnect(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        crate::credentials::store(&self.home, None)?;
        self.cache.clear()?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        *s = State {
            loaded: true,
            ..Default::default()
        };
        Ok(())
    }
    pub fn cancel(&self) {
        self.state.lock().unwrap().pending = None;
    }
}
