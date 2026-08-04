use serde::Serialize;
use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntimeKind {
    Native,
    Tmux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPaneKind {
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTabContext {
    pub index: usize,
    pub title: String,
    pub pane_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPaneContext {
    pub index: usize,
    pub kind: PluginPaneKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
    pub selected_text_truncated: bool,
    pub shell: String,
    pub runtime: PluginRuntimeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<PluginTabContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pane: Option<PluginPaneContext>,
    pub platform: String,
    pub app_version: String,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_serializes_the_public_camel_case_contract() {
        let value = serde_json::to_value(PluginContext {
            working_directory: Some("/repo".to_string()),
            active_command: Some("cargo test".to_string()),
            selected_text: Some("failed assertion".to_string()),
            selected_text_truncated: false,
            shell: "/bin/zsh".to_string(),
            runtime: PluginRuntimeKind::Tmux,
            active_tab: Some(PluginTabContext {
                index: 2,
                title: "tests".to_string(),
                pane_count: 3,
            }),
            active_pane: Some(PluginPaneContext {
                index: 1,
                kind: PluginPaneKind::Terminal,
            }),
            platform: "macos".to_string(),
            app_version: "1.2.3".to_string(),
            settings: BTreeMap::new(),
        })
        .expect("serialize plugin context");

        assert_eq!(
            value,
            json!({
                "workingDirectory": "/repo",
                "activeCommand": "cargo test",
                "selectedText": "failed assertion",
                "selectedTextTruncated": false,
                "shell": "/bin/zsh",
                "runtime": "tmux",
                "activeTab": { "index": 2, "title": "tests", "paneCount": 3 },
                "activePane": { "index": 1, "kind": "terminal" },
                "platform": "macos",
                "appVersion": "1.2.3",
                "settings": {},
            })
        );
    }

    #[test]
    fn unavailable_optional_context_is_omitted() {
        let value = serde_json::to_value(PluginContext {
            working_directory: None,
            active_command: None,
            selected_text: None,
            selected_text_truncated: false,
            shell: "/bin/bash".to_string(),
            runtime: PluginRuntimeKind::Native,
            active_tab: None,
            active_pane: None,
            platform: "linux".to_string(),
            app_version: "test".to_string(),
            settings: BTreeMap::new(),
        })
        .expect("serialize plugin context");

        assert!(value.get("workingDirectory").is_none());
        assert!(value.get("activeCommand").is_none());
        assert!(value.get("selectedText").is_none());
        assert!(value.get("activeTab").is_none());
        assert!(value.get("activePane").is_none());
    }
}
