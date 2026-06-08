//! Host-side persistence for WiFi authentication tokens.
//!
//! Tokens are stored as JSON and upserted by host identifier. The default path
//! is the first available base directory from `XDG_CONFIG_HOME/enody`,
//! `HOME/.enody`, `USERPROFILE/.enody`, or `APPDATA/enody`.

use crate::message::Token;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Saved WiFi authentication tokens.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct TokenStore {
    #[serde(default)]
    tokens: Vec<Token>,
}

impl TokenStore {
    /// Returns the configuration directory used for token persistence.
    pub fn config_dir() -> Result<PathBuf, crate::Error> {
        let config_home = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty());
        if let Some(config_home) = config_home {
            return Ok(PathBuf::from(config_home).join("enody"));
        }

        let home = env::var_os("HOME").filter(|value| !value.is_empty());
        if let Some(home) = home {
            return Ok(PathBuf::from(home).join(".enody"));
        }

        let userprofile = env::var_os("USERPROFILE").filter(|value| !value.is_empty());
        if let Some(userprofile) = userprofile {
            return Ok(PathBuf::from(userprofile).join(".enody"));
        }

        let appdata = env::var_os("APPDATA").filter(|value| !value.is_empty());
        if let Some(appdata) = appdata {
            return Ok(PathBuf::from(appdata).join("enody"));
        }

        Err(crate::Error::Debug(
            "XDG_CONFIG_HOME, HOME, APPDATA, or USERPROFILE is required".to_string(),
        ))
    }

    /// Returns the default token JSON path.
    pub fn path() -> Result<PathBuf, crate::Error> {
        Ok(Self::config_dir()?.join("tokens.json"))
    }

    /// Loads the token store from the default path.
    pub fn load() -> Result<Self, crate::Error> {
        Self::load_from_path(Self::path()?)
    }

    /// Loads a token store from a specific path.
    ///
    /// Missing files produce an empty store.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, crate::Error> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(crate::Error::Debug(error.to_string())),
        };
        serde_json::from_str(&contents).map_err(|error| crate::Error::Debug(error.to_string()))
    }

    /// Saves the token store to the default path and returns that path.
    pub fn save(&self) -> Result<PathBuf, crate::Error> {
        let path = Self::path()?;
        self.save_to_path(&path)?;
        Ok(path)
    }

    /// Saves the token store to a specific path.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), crate::Error> {
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| crate::Error::Debug(error.to_string()))?;
        }

        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options
            .open(path)
            .map_err(|error| crate::Error::Debug(error.to_string()))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| crate::Error::Debug(error.to_string()))
    }

    /// Loads the default store, upserts one token, saves it, and returns the path.
    pub fn save_token(token: &Token) -> Result<PathBuf, crate::Error> {
        let mut store = Self::load()?;
        store.upsert(token.clone());
        store.save()
    }

    /// Returns the saved tokens.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// Consumes the store and returns the saved tokens.
    pub fn into_tokens(self) -> Vec<Token> {
        self.tokens
    }

    /// Inserts or replaces a token by host identifier.
    pub fn upsert(&mut self, token: Token) {
        if let Some(existing) = self
            .tokens
            .iter_mut()
            .find(|existing| existing.host_id == token.host_id)
        {
            *existing = token;
        } else {
            self.tokens.push(token);
        }
    }
}
