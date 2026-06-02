use crate::message::Token;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct TokenStore {
    #[serde(default)]
    tokens: Vec<Token>,
}

impl TokenStore {
    pub fn config_dir() -> Result<PathBuf, crate::Error> {
        let config_home = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty());
        if let Some(config_home) = config_home {
            return Ok(PathBuf::from(config_home).join("enody"));
        }

        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                crate::Error::Debug("XDG_CONFIG_HOME or HOME is required".to_string())
            })?;
        Ok(PathBuf::from(home).join(".enody"))
    }

    pub fn path() -> Result<PathBuf, crate::Error> {
        Ok(Self::config_dir()?.join("tokens.json"))
    }

    pub fn load() -> Result<Self, crate::Error> {
        Self::load_from_path(Self::path()?)
    }

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

    pub fn save(&self) -> Result<PathBuf, crate::Error> {
        let path = Self::path()?;
        self.save_to_path(&path)?;
        Ok(path)
    }

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

    pub fn save_token(token: &Token) -> Result<PathBuf, crate::Error> {
        let mut store = Self::load()?;
        store.upsert(token.clone());
        store.save()
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn into_tokens(self) -> Vec<Token> {
        self.tokens
    }

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
