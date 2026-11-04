use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct ProviderProfile {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
}

pub struct Resolved {
    pub profile: ProviderProfile,
    pub api_key: String,
}

pub fn resolve(home: &Path) -> Result<Resolved> {
    choose(home, &ProcessEnv)
}

pub fn choose(home: &Path, env: &dyn Env) -> Result<Resolved> {
    let stored = stored(home)?;
    if stored.is_none() {
        if nonempty(env.get("CONVERGE_PROVIDER")).is_none() {
            return env_provider(home, env);
        }
    }
    let (providers, active) = view(stored.as_ref());
    let selected = nonempty(env.get("CONVERGE_PROVIDER")).unwrap_or(active);
    let profile = providers
        .into_iter()
        .find(|profile| profile.name == selected)
        .ok_or_else(|| Error::new(format!("provider {selected} is not configured")))?;
    finish(home, profile, env)
}

pub fn list(home: &Path) -> Result<String> {
    let stored = stored(home)?;
    let (providers, active) = view(stored.as_ref());
    let mut lines = Vec::new();
    for profile in providers {
        let mark = if profile.name == active { "active" } else { "" };
        lines.push(format!(
            "{}\t{}\t{}\t{mark}",
            profile.name, profile.model, profile.base_url
        ));
    }
    Ok(lines.join("\n"))
}

pub fn store_api_key(home: &Path, name: &str, value: &str) -> Result<()> {
    crate::secret::write_secret(home, name, value)
}

pub fn configure_cli(home: &Path, name: &str) -> Result<()> {
    configure_provider(home, name, &ProcessEnv)
}

pub fn configure_provider(home: &Path, name: &str, env: &dyn Env) -> Result<()> {
    let (providers, _) = profiles(home)?;
    let profile = providers
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| Error::new(format!("provider {name} is not configured")))?;
    if profile.api_key_env.is_empty() {
        return use_provider(home, name);
    }
    let saved = crate::secret::read_secret(home, &profile.api_key_env)?;
    let from_env = nonempty(env.get(&profile.api_key_env));
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        if saved.is_some() || from_env.is_some() {
            return use_provider(home, name);
        }
        return Err(missing_key(&profile.api_key_env));
    }
    if let Some(existing) = saved.as_ref().or(from_env.as_ref()) {
        println!(
            "An API key for {} is already available ({}).",
            profile.name,
            hint(existing)
        );
        print!("[K]eep / [R]eplace (default K): ");
        std::io::Write::flush(&mut std::io::stdout())
            .map_err(|error| Error::new(error.to_string()))?;
        let choice = crate::secret::read_reply()?;
        if !choice.to_ascii_lowercase().starts_with('r') {
            return use_provider(home, name);
        }
    } else {
        println!("No {} API key configured.", profile.name);
    }
    let entered = crate::secret::prompt_secret(&format!(
        "API key for {} (or Enter to cancel): ",
        profile.name
    ))?;
    if entered.is_empty() {
        return Err(Error::new("cancelled"));
    }
    crate::secret::write_secret(home, &profile.api_key_env, &entered)?;
    use_provider(home, name)?;
    println!("API key saved.");
    Ok(())
}

pub(crate) fn profiles(home: &Path) -> Result<(Vec<ProviderProfile>, String)> {
    let stored = stored(home)?;
    Ok(view(stored.as_ref()))
}

pub fn use_provider(home: &Path, name: &str) -> Result<()> {
    let path = registry_path(home);
    let stored = stored(home)?;
    let (providers, _) = view(stored.as_ref());
    if !providers.iter().any(|profile| profile.name == name) {
        return Err(Error::new(format!("provider {name} is not configured")));
    }
    let mut registry = stored.unwrap_or(Registry {
        active: String::new(),
        providers: Vec::new(),
    });
    registry.active = name.to_string();
    write(&path, &registry)
}

pub fn secret_envs(home: &Path) -> Vec<String> {
    let mut names = vec!["CONVERGE_API_KEY".to_string()];
    for profile in catalog() {
        push_secret(&mut names, profile.api_key_env);
    }
    if let Ok(Some(registry)) = stored(home) {
        for profile in registry.providers {
            push_secret(&mut names, profile.api_key_env);
        }
    }
    names
}

pub trait Env {
    fn get(&self, name: &str) -> Option<String>;
}

struct ProcessEnv;

impl Env for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

struct Registry {
    active: String,
    providers: Vec<ProviderProfile>,
}

fn catalog() -> Vec<ProviderProfile> {
    vec![
        builtin(
            "anthropic",
            "https://api.anthropic.com/v1",
            "claude-opus-5-5",
            "ANTHROPIC_API_KEY",
        ),
        builtin(
            "openai",
            "https://api.openai.com/v1",
            "gpt-6-astra",
            "OPENAI_API_KEY",
        ),
        builtin("ollama", "http://127.0.0.1:11434/v1", "llama3.2", ""),
        builtin(
            "alibaba",
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            "qwen3.8-max",
            "DASHSCOPE_API_KEY",
        ),
        builtin("grok", "https://api.x.ai/v1", "grok-4.7", "XAI_API_KEY"),
        builtin(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "openrouter/auto",
            "OPENROUTER_API_KEY",
        ),
        builtin(
            "cerebras",
            "https://api.cerebras.ai/v1",
            "qwen-3.8-27b",
            "CEREBRAS_API_KEY",
        ),
        builtin(
            "venice",
            "https://api.venice.ai/api/v1",
            "default",
            "VENICE_API_KEY",
        ),
        builtin(
            "deepseek",
            "https://api.deepseek.com",
            "deepseek-flash",
            "DEEPSEEK_API_KEY",
        ),
        builtin(
            "kimi",
            "https://api.moonshot.ai/v1",
            "kimi-k3.1-pro",
            "MOONSHOT_API_KEY",
        ),
        builtin(
            "gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-3.8-flash",
            "GEMINI_API_KEY",
        ),
    ]
}

fn builtin(name: &str, base_url: &str, model: &str, api_key_env: &str) -> ProviderProfile {
    ProviderProfile {
        name: name.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        api_key_env: api_key_env.to_string(),
    }
}

fn stored(home: &Path) -> Result<Option<Registry>> {
    let path = registry_path(home);
    if path.is_file() {
        Ok(Some(load(&path)?))
    } else {
        Ok(None)
    }
}

fn view(stored: Option<&Registry>) -> (Vec<ProviderProfile>, String) {
    let mut providers = catalog();
    let active = match stored {
        Some(registry) => {
            for extra in &registry.providers {
                if let Some(slot) = providers
                    .iter_mut()
                    .find(|profile| profile.name == extra.name)
                {
                    *slot = extra.clone();
                } else {
                    providers.push(extra.clone());
                }
            }
            registry.active.clone()
        }
        None => String::new(),
    };
    (providers, active)
}

fn finish(home: &Path, profile: ProviderProfile, env: &dyn Env) -> Result<Resolved> {
    let model = nonempty(env.get("CONVERGE_MODEL")).unwrap_or(profile.model);
    let profile = ProviderProfile { model, ..profile };
    check_url(&profile.base_url)?;
    let api_key = if profile.api_key_env.is_empty() {
        String::new()
    } else {
        match nonempty(env.get(&profile.api_key_env)) {
            Some(key) => key,
            None => crate::secret::read_secret(home, &profile.api_key_env)?
                .ok_or_else(|| missing_key(&profile.api_key_env))?,
        }
    };
    Ok(Resolved { profile, api_key })
}

fn missing_key(name: &str) -> Error {
    Error::new(format!(
        "{name} is not configured. Run converge setup in a terminal and enter it."
    ))
}

fn hint(value: &str) -> String {
    let count = value.chars().count();
    if count <= 8 {
        "...".to_string()
    } else {
        let prefix: String = value.chars().take(8).collect();
        format!("{prefix}...")
    }
}

fn push_secret(names: &mut Vec<String>, name: String) {
    if !name.is_empty() && !names.iter().any(|item| item == &name) {
        names.push(name);
    }
}

fn env_provider(home: &Path, env: &dyn Env) -> Result<Resolved> {
    let base_url = nonempty(env.get("CONVERGE_BASE_URL"))
        .ok_or_else(|| Error::new("CONVERGE_BASE_URL is required"))?;
    let model = nonempty(env.get("CONVERGE_MODEL"))
        .ok_or_else(|| Error::new("CONVERGE_MODEL is required"))?;
    let api_key = match nonempty(env.get("CONVERGE_API_KEY")) {
        Some(key) => key,
        None => crate::secret::read_secret(home, "CONVERGE_API_KEY")?
            .ok_or_else(|| missing_key("CONVERGE_API_KEY"))?,
    };
    check_url(&base_url)?;
    Ok(Resolved {
        profile: ProviderProfile {
            name: "env".to_string(),
            base_url,
            model,
            api_key_env: "CONVERGE_API_KEY".to_string(),
        },
        api_key,
    })
}

fn load(path: &Path) -> Result<Registry> {
    let bytes =
        fs::read(path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(Error::new("providers.json schema version must be 1"));
    }
    let active = value
        .get("active")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let entries = value
        .get("providers")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("providers.json providers must be an array"))?;
    let mut providers = Vec::new();
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Err(Error::new("provider name is empty"));
        }
        if providers
            .iter()
            .any(|profile: &ProviderProfile| profile.name == name)
        {
            return Err(Error::new(format!("provider {name} is duplicated")));
        }
        let base_url = entry
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let model = entry
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if base_url.is_empty() || model.is_empty() {
            return Err(Error::new(format!(
                "provider {name} needs a base_url and a model"
            )));
        }
        check_url(&base_url)?;
        let api_key_env = entry
            .get("api_key_env")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("CONVERGE_API_KEY")
            .to_string();
        providers.push(ProviderProfile {
            name,
            base_url,
            model,
            api_key_env,
        });
    }
    let known = providers.iter().any(|profile| profile.name == active)
        || catalog().iter().any(|profile| profile.name == active);
    if !known {
        return Err(Error::new(format!("provider {active} is not configured")));
    }
    Ok(Registry { active, providers })
}

fn write(path: &Path, registry: &Registry) -> Result<()> {
    let providers: Vec<Value> = registry
        .providers
        .iter()
        .map(|profile| {
            json!({
                "name": profile.name,
                "base_url": profile.base_url,
                "model": profile.model,
                "api_key_env": profile.api_key_env,
            })
        })
        .collect();
    let value = json!({
        "version": 1,
        "active": registry.active,
        "providers": providers,
    });
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| Error::new(format!("encode providers: {error}")))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    }
    fs::write(path, bytes).map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

fn registry_path(home: &Path) -> PathBuf {
    home.join("providers.json")
}

fn check_url(base_url: &str) -> Result<()> {
    if base_url.starts_with("https://") || base_url.starts_with("http://") {
        Ok(())
    } else {
        Err(Error::new("provider base_url must be an http or https URL"))
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.is_empty())
}
