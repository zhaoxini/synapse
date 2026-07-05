//! Model catalog: the models a client may pick, derived from Claude Code's own
//! config plus optional user customizations.
//!
//! Synapse only shells out to `claude -p --model <id>`, and Claude Code itself
//! resolves the family aliases (`opus`/`sonnet`/`haiku`) through its settings
//! `env` (`ANTHROPIC_DEFAULT_<ALIAS>_MODEL`). So the catalog is just a labeled
//! id list: the id is what we pass to `--model` (empty = omit the flag and let
//! Claude Code use its configured default), the label is what the picker shows.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingInfo {
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub can_disable: bool,
    #[serde(default)]
    pub adaptive: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Passed to `claude --model`. Empty = omit the flag (Claude Code default).
    pub id: String,
    /// Human label shown in the picker.
    pub label: String,
    /// Effort levels supported by this model, as understood by Claude Code.
    #[serde(default, rename = "effortLevels")]
    pub effort_levels: Vec<String>,
    /// Thinking controls supported by this model.
    #[serde(default)]
    pub thinking: ThinkingInfo,
    /// Optional custom capability declarations from ~/.synapse/models.json.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Build the catalog and the default model id from on-disk config.
///
/// Catalog = `Default` + `opus`/`sonnet`/`haiku` (labels from Claude Code's
/// `ANTHROPIC_DEFAULT_*_MODEL_NAME` when present) + custom entries from
/// `~/.synapse/models.json` (`[{"id","label"}]`). The default is
/// `default_override` (synapse `--default-model`) if set, else Claude Code's
/// configured `model`, else empty (the `Default` entry).
pub fn discover_catalog(default_override: Option<String>) -> (Vec<ModelInfo>, String) {
    build_catalog(
        read_claude_settings().as_ref(),
        read_custom_models(),
        default_override,
    )
}

fn build_catalog(
    settings: Option<&serde_json::Value>,
    custom: Vec<ModelInfo>,
    default_override: Option<String>,
) -> (Vec<ModelInfo>, String) {
    let label_for = |alias: &str, fallback: &str| -> String {
        settings
            .and_then(|s| s.get("env"))
            .and_then(|e| {
                e.get(format!("ANTHROPIC_DEFAULT_{}_MODEL_NAME", alias.to_uppercase()).as_str())
            })
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| fallback.to_string())
    };

    let mut catalog = vec![
        model_info(String::new(), "Default".into(), Vec::new()),
        model_info("opus".into(), label_for("opus", "Opus"), Vec::new()),
        model_info("sonnet".into(), label_for("sonnet", "Sonnet"), Vec::new()),
        model_info("haiku".into(), label_for("haiku", "Haiku"), Vec::new()),
        model_info("fable".into(), label_for("fable", "Fable"), Vec::new()),
    ];
    // Custom entries augment the catalog; skip ids already present.
    for m in custom {
        if !m.id.is_empty() && !catalog.iter().any(|c| c.id == m.id) {
            catalog.push(model_info(m.id, m.label, m.capabilities));
        }
    }

    let default = default_override
        .filter(|s| !s.is_empty())
        .or_else(|| {
            settings
                .and_then(|s| s.get("model"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();

    // The configured default must itself be selectable + highlightable, even
    // when it's a full id (e.g. `--default-model claude-sonnet-4-6`) outside the
    // alias set.
    if !default.is_empty() && !catalog.iter().any(|c| c.id == default) {
        catalog.push(model_info(default.clone(), default.clone(), Vec::new()));
    }

    (catalog, default)
}

fn model_info(id: String, label: String, capabilities: Vec<String>) -> ModelInfo {
    let (effort_levels, thinking) = if capabilities.is_empty() {
        inferred_capabilities(&id)
    } else {
        declared_capabilities(&id, &capabilities)
    };
    ModelInfo {
        id,
        label,
        effort_levels,
        thinking,
        capabilities,
    }
}

fn inferred_capabilities(id: &str) -> (Vec<String>, ThinkingInfo) {
    let id = id.to_ascii_lowercase();
    let levels_x = levels(&["low", "medium", "high", "xhigh", "max"]);
    let levels_no_x = levels(&["low", "medium", "high", "max"]);
    if id == "fable" || id.contains("fable-5") {
        return (levels_x, thinking(true, false, true));
    }
    if id == "opus" || id.contains("opus-4-8") || id.contains("opus-4-7") {
        return (levels_x, thinking(true, true, true));
    }
    if id.contains("opus-4-6") {
        return (levels_no_x, thinking(true, true, true));
    }
    if id == "sonnet" || id.contains("sonnet-5") {
        return (levels_x, thinking(true, true, true));
    }
    if id.contains("sonnet-4-6") {
        return (levels_no_x, thinking(true, true, true));
    }
    (Vec::new(), ThinkingInfo::default())
}

fn declared_capabilities(id: &str, caps: &[String]) -> (Vec<String>, ThinkingInfo) {
    let has = |needle: &str| caps.iter().any(|c| c == needle);
    let mut effort = Vec::new();
    if has("effort") {
        effort = levels(&["low", "medium", "high"]);
        if has("xhigh_effort") {
            effort.push("xhigh".into());
        }
        if has("max_effort") {
            effort.push("max".into());
        }
    }
    let is_fable = id.eq_ignore_ascii_case("fable") || id.to_ascii_lowercase().contains("fable-5");
    let thinking = ThinkingInfo {
        supported: has("thinking"),
        can_disable: has("thinking") && !is_fable,
        adaptive: has("adaptive_thinking"),
    };
    (effort, thinking)
}

fn levels(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|x| (*x).to_string()).collect()
}

fn thinking(supported: bool, can_disable: bool, adaptive: bool) -> ThinkingInfo {
    ThinkingInfo {
        supported,
        can_disable,
        adaptive,
    }
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn read_claude_settings() -> Option<serde_json::Value> {
    let path = home()?.join(".claude").join("settings.json");
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn read_custom_models() -> Vec<ModelInfo> {
    home()
        .and_then(|h| std::fs::read_to_string(h.join(".synapse").join("models.json")).ok())
        .and_then(|t| serde_json::from_str::<Vec<ModelInfo>>(&t).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_merges_labels_custom_and_default() {
        let settings = json!({
            "model": "haiku",
            "env": {
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "claude-opus-4-8",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "glm-5.3-external"
            }
        });
        let custom = vec![
            ModelInfo {
                id: "gpt-5.5".into(),
                label: "GPT-5.5".into(),
                ..Default::default()
            },
            ModelInfo {
                id: "opus".into(),
                label: "dup-ignored".into(),
                ..Default::default()
            },
        ];

        let (cat, def) = build_catalog(Some(&settings), custom, None);

        assert_eq!(def, "haiku", "default falls back to Claude Code `model`");
        assert_eq!(cat[0].id, "", "Default entry is first and omits --model");
        assert_eq!(cat[1].label, "claude-opus-4-8", "opus label from env");
        assert_eq!(
            cat[2].label, "Sonnet",
            "sonnet label falls back when env absent"
        );
        assert_eq!(cat[3].label, "glm-5.3-external", "haiku label from env");
        assert!(
            cat.iter().any(|m| m.id == "fable"),
            "fable alias is offered"
        );
        assert_eq!(
            cat.iter().find(|m| m.id == "opus").unwrap().effort_levels,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            cat.iter()
                .find(|m| m.id == "haiku")
                .unwrap()
                .effort_levels
                .len(),
            0,
            "haiku has no documented effort levels"
        );
        assert_eq!(
            cat.iter()
                .find(|m| m.id == "fable")
                .unwrap()
                .thinking
                .can_disable,
            false,
            "fable thinking cannot be disabled"
        );
        assert!(
            cat.iter().any(|m| m.id == "gpt-5.5"),
            "custom model appended"
        );
        assert_eq!(
            cat.iter().filter(|m| m.id == "opus").count(),
            1,
            "duplicate custom id is dropped"
        );

        // synapse override beats Claude Code's configured model.
        let (_, def2) = build_catalog(Some(&settings), vec![], Some("sonnet".into()));
        assert_eq!(def2, "sonnet");

        // a full-id override outside the alias set is added as a catalog entry.
        let (cat2, def_full) =
            build_catalog(Some(&settings), vec![], Some("claude-sonnet-4-6".into()));
        assert_eq!(def_full, "claude-sonnet-4-6");
        assert!(cat2.iter().any(|m| m.id == "claude-sonnet-4-6"));

        // no settings, no override → empty default (the Default entry).
        let (_, def3) = build_catalog(None, vec![], None);
        assert_eq!(def3, "");
    }
}
