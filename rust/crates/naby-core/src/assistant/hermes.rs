//! Hermes Agent connection rules and copyable setup text (`AssistantHermesSetup.cs`).
//!
//! Hermes keeps its own tools, skills, memory and permissions; Naby only needs its API address,
//! the `API_SERVER_KEY` (stored in the credential manager) and an advertised agent id.

use super::models::Provider;
use super::providers::{create, escape_data_string, validate_endpoint, PRESETS};
use super::{AssistantError, Result};

pub const DEFAULT_ADDRESS: &str = "http://127.0.0.1:8642/v1/";
pub const SETUP_GUIDE_URL: &str =
    "https://hermes-agent.nousresearch.com/docs/user-guide/features/api-server/";
/// The gateway template the setup page copies. The key placeholder is never filled in.
pub const GATEWAY_TEMPLATE: &str = "API_SERVER_ENABLED=true\nAPI_SERVER_KEY=<your-own-long-random-key>\n\n# Start or restart after saving the profile .env:\nhermes gateway";
pub const MAXIMUM_AGENT_ID_CHARACTERS: usize = 160;
pub const MAXIMUM_PROFILE_CHARACTERS: usize = 64;

/// The API base URL for the address and optional profile typed in the setup page (`BaseUrl`).
pub fn base_url(address: &str, profile: &str) -> Result<String> {
    let mut value = address.trim().trim_end_matches('/').to_string();
    let name = profile.trim();
    let lower = |s: &str| s.to_ascii_lowercase();
    if !name.is_empty() {
        if name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            return Err(AssistantError::invalid(
                "Use letters, numbers, hyphens or underscores for the profile name.",
            ));
        }
        let uri = url::Url::parse(&value)
            .map_err(|_| AssistantError::invalid("Enter the Hermes API address."))?;
        if lower(uri.path()).contains("/p/") {
            return Err(AssistantError::invalid(
                "The address already includes a profile. Leave Profile empty.",
            ));
        }
        if lower(&value).ends_with("/v1") {
            value.truncate(value.len() - 3);
        }
        value.push_str(&format!("/p/{}/v1", escape_data_string(name)));
    } else if !lower(&value).ends_with("/v1") {
        value.push_str("/v1");
    }
    Ok(validate_endpoint(&value)?.to_string())
}

/// The saved Hermes connection for exactly this address, if any (`Existing`). A saved key is only
/// reused for the exact same address/profile.
pub fn existing<'a>(providers: &'a [Provider], endpoint: &str) -> Option<&'a Provider> {
    providers.iter().find(|p| {
        p.is_hermes() && p.base_url.trim_end_matches('/') == endpoint.trim_end_matches('/')
    })
}

/// Pre-select the agent after "Check server": keep the typed one if advertised, else the only one.
pub fn choose_agent(typed: &str, models: &[String]) -> String {
    let typed = typed.trim();
    if models.iter().any(|m| m == typed) {
        typed.to_string()
    } else if models.len() == 1 {
        models[0].clone()
    } else {
        String::new()
    }
}

/// The provider saved by "Connect & use Hermes".
pub fn connection(endpoint: &str, agent: &str, profile: &str, images: bool) -> Result<Provider> {
    let agent = agent.trim();
    if agent.is_empty() || agent.encode_utf16().count() > MAXIMUM_AGENT_ID_CHARACTERS {
        return Err(AssistantError::invalid(
            "Choose or paste an advertised agent ID first.",
        ));
    }
    let mut provider = create(PRESET)?;
    provider.base_url = endpoint.into();
    provider.model = agent.into();
    let profile = profile.trim();
    provider.name = if profile.is_empty() {
        "Hermes Agent".into()
    } else {
        format!("Hermes · {profile}")
    };
    provider.vision = images;
    Ok(provider)
}

/// Whether a new Hermes connection fits in the provider list.
pub fn check_room(providers: &[Provider]) -> Result<()> {
    if providers.len() >= super::models::MAX_PROVIDERS {
        return Err(AssistantError::invalid(
            "Remove an unused provider before adding Hermes.",
        ));
    }
    Ok(())
}

/// `C:\Program Files\NabyScape\NabyScape.exe` as WSL sees it.
pub fn wsl_path(executable: &str) -> Result<String> {
    let bytes = executable.as_bytes();
    if executable.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return Err(AssistantError::io(
            "Use the Windows setup for this installation path.",
        ));
    }
    Ok(format!(
        "/mnt/{}{}",
        (bytes[0] as char).to_ascii_lowercase(),
        executable[2..].replace('\\', "/")
    ))
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// The `mcp_servers` entry to merge into Hermes' `config.yaml`.
pub fn mcp_yaml(executable: &str) -> String {
    format!(
        "mcp_servers:\n  nabyscape:\n    command: {}\n    args: [\"--widget-mcp\"]\n",
        json_string(executable)
    )
}

/// The `mcpServers` JSON for other MCP clients.
pub fn mcp_json(executable: &str) -> String {
    format!(
        "{{\n  \"mcpServers\": {{\n    \"nabyscape\": {{\n      \"command\": {},\n      \"args\": [\n        \"--widget-mcp\"\n      ]\n    }}\n  }}\n}}",
        json_string(executable)
    )
}

/// The preset name used for Hermes in [`PRESETS`].
pub const PRESET: &str = PRESETS[6];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized_to_the_v1_root() {
        assert_eq!(
            base_url("http://127.0.0.1:8642", "").unwrap(),
            "http://127.0.0.1:8642/v1/"
        );
        assert_eq!(
            base_url(" http://127.0.0.1:8642/v1/ ", "").unwrap(),
            "http://127.0.0.1:8642/v1/"
        );
        assert_eq!(
            base_url("http://127.0.0.1:8642/v1", "work-1").unwrap(),
            "http://127.0.0.1:8642/p/work-1/v1/"
        );
        assert_eq!(
            base_url("https://hermes.example.com", "My_Profile").unwrap(),
            "https://hermes.example.com/p/My_Profile/v1/"
        );
        assert!(base_url("http://127.0.0.1:8642/p/x/v1", "y").is_err());
        assert!(base_url("http://127.0.0.1:8642", "a b").is_err());
        assert!(base_url("http://127.0.0.1:8642", "a/b").is_err());
        assert!(
            base_url("http://hermes.example.com", "").is_err(),
            "cleartext remote"
        );
        assert!(base_url("https://10.0.0.5", "").is_err(), "private address");
        assert!(base_url("not a url", "p").is_err());
    }

    #[test]
    fn connections_reuse_the_exact_saved_address() {
        let mut saved = create("Hermes Agent").unwrap();
        saved.base_url = "http://127.0.0.1:8642/v1/".into();
        let other = create("Ollama").unwrap();
        let list = vec![other, saved.clone()];
        assert_eq!(
            existing(&list, "http://127.0.0.1:8642/v1").unwrap().id,
            saved.id
        );
        assert!(existing(&list, "http://127.0.0.1:8642/p/x/v1/").is_none());
        let p = connection("http://127.0.0.1:8642/p/x/v1/", " agent ", "x", true).unwrap();
        assert_eq!(
            (
                p.name.as_str(),
                p.model.as_str(),
                p.vision,
                p.backend.as_str()
            ),
            ("Hermes · x", "agent", true, "Hermes")
        );
        assert!(!p.tools);
        assert_eq!(
            connection("http://127.0.0.1:8642/v1/", "a", "", false)
                .unwrap()
                .name,
            "Hermes Agent"
        );
        assert!(connection("http://127.0.0.1:8642/v1/", " ", "", false).is_err());
        assert!(connection("http://127.0.0.1:8642/v1/", &"a".repeat(161), "", false).is_err());
        let models = vec!["hermes-agent".to_string(), "work".to_string()];
        assert_eq!(choose_agent("work", &models), "work");
        assert_eq!(choose_agent("zzz", &models), "");
        assert_eq!(choose_agent("zzz", &models[..1]), "hermes-agent");
        assert!(check_room(&list).is_ok());
        assert!(check_room(&vec![saved; 12]).is_err());
        assert_eq!(PRESET, "Hermes Agent");
    }

    #[test]
    fn setup_text_matches_the_copy_buttons() {
        assert!(GATEWAY_TEMPLATE.starts_with("API_SERVER_ENABLED=true\nAPI_SERVER_KEY=<your-own"));
        let exe = r"C:\Program Files\NabyScape\NabyScape.exe";
        assert_eq!(
            mcp_yaml(exe),
            "mcp_servers:\n  nabyscape:\n    command: \"C:\\\\Program Files\\\\NabyScape\\\\NabyScape.exe\"\n    args: [\"--widget-mcp\"]\n"
        );
        assert_eq!(
            wsl_path(exe).unwrap(),
            "/mnt/c/Program Files/NabyScape/NabyScape.exe"
        );
        assert!(wsl_path("/usr/bin/x").is_err());
        let json: serde_json::Value = serde_json::from_str(&mcp_json(exe)).unwrap();
        assert_eq!(json["mcpServers"]["nabyscape"]["command"], exe);
        assert_eq!(json["mcpServers"]["nabyscape"]["args"][0], "--widget-mcp");
    }
}
