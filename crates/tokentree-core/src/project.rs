// SPDX-License-Identifier: Apache-2.0
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDetectionMethod {
    Override,
    VerifiedMapping,
    Config,
    Git,
    Manifest,
    Cwd,
    PersonalInbox,
}

impl ProjectDetectionMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::VerifiedMapping => "verified_mapping",
            Self::Config => "config",
            Self::Git => "git",
            Self::Manifest => "manifest",
            Self::Cwd => "cwd",
            Self::PersonalInbox => "personal_inbox",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectCandidate {
    pub key: String,
    pub display_name: String,
    pub root: PathBuf,
    pub method: ProjectDetectionMethod,
    pub confidence: f64,
}

#[derive(Clone, Debug, Default)]
pub struct ResolveProjectInput {
    pub cwd: PathBuf,
    pub override_key: Option<String>,
    pub override_title: Option<String>,
    pub verified_key: Option<String>,
    pub verified_title: Option<String>,
    pub home: Option<PathBuf>,
}

#[must_use]
pub fn format_title(value: &str) -> String {
    let unbracketed = if let Some(stripped) = value.strip_prefix('@') {
        stripped.split_once('/').map_or(stripped, |(_, rest)| rest)
    } else {
        value
    };
    let spaced = unbracketed.replace(['-', '_'], " ");
    let mut words = Vec::new();
    for word in spaced.split_whitespace() {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            let capitalized: String = first.to_uppercase().chain(chars).collect();
            words.push(capitalized);
        }
    }
    if words.is_empty() {
        "Personal Unassigned".into()
    } else {
        words.join(" ")
    }
}

#[must_use]
pub fn slug_key(value: &str) -> String {
    let mut result = String::new();
    let mut last_was_dash = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !result.is_empty() {
            result.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = result.trim_matches('-');
    if trimmed.is_empty() {
        "personal-unassigned".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn check_untrusted_config(text: &str) -> Result<()> {
    for line in text.lines() {
        let trimmed = line.trim();
        for forbidden in [
            "command:", "exec:", "hook:", "egress:", "network:", "pricing:",
        ] {
            if trimmed.starts_with(forbidden) {
                bail!("Untrusted .tokentree.yml contains forbidden capability keys");
            }
        }
    }
    Ok(())
}

fn config_at(dir: &Path) -> Result<Option<(String, Option<String>)>> {
    let config_path = dir.join(".tokentree.yml");
    if !config_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&config_path)?;
    check_untrusted_config(&content)?;

    let mut project_key = None;
    let mut project_title = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("project:") {
            let val = rest.split('#').next().unwrap_or(rest).trim();
            if !val.is_empty() {
                project_key = Some(val.to_owned());
            }
        } else if let Some(rest) = trimmed.strip_prefix("title:") {
            let val = rest.split('#').next().unwrap_or(rest).trim();
            if !val.is_empty() {
                project_title = Some(val.to_owned());
            }
        }
    }

    Ok(project_key.map(|key| (key, project_title)))
}

pub fn resolve_project(input: ResolveProjectInput) -> Result<ProjectCandidate> {
    let cwd = if input.cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        input.cwd.canonicalize().unwrap_or(input.cwd)
    };
    let home = input
        .home
        .or_else(dirs::home_dir)
        .map(|h| h.canonicalize().unwrap_or(h));

    if let Some(key) = input.override_key {
        let display = input.override_title.unwrap_or_else(|| format_title(&key));
        return Ok(ProjectCandidate {
            key: slug_key(&key),
            display_name: display,
            root: cwd,
            method: ProjectDetectionMethod::Override,
            confidence: 1.0,
        });
    }

    if let Some(key) = input.verified_key {
        let display = input.verified_title.unwrap_or_else(|| format_title(&key));
        return Ok(ProjectCandidate {
            key: slug_key(&key),
            display_name: display,
            root: cwd,
            method: ProjectDetectionMethod::VerifiedMapping,
            confidence: 1.0,
        });
    }

    // 1. Walk upward checking .tokentree.yml
    let mut current = Some(cwd.as_path());
    while let Some(dir) = current {
        if let Some((k, t)) = config_at(dir)? {
            let display = t.unwrap_or_else(|| format_title(&k));
            return Ok(ProjectCandidate {
                key: slug_key(&k),
                display_name: display,
                root: dir.to_path_buf(),
                method: ProjectDetectionMethod::Config,
                confidence: 0.98,
            });
        }
        current = dir.parent();
    }

    // 2. Walk upward looking for .git (excluding home directory)
    current = Some(cwd.as_path());
    while let Some(dir) = current {
        let is_home = home.as_deref() == Some(dir);
        if !is_home && dir.join(".git").exists() {
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            return Ok(ProjectCandidate {
                key: slug_key(name),
                display_name: format_title(name),
                root: dir.to_path_buf(),
                method: ProjectDetectionMethod::Git,
                confidence: 0.92,
            });
        }
        if is_home {
            break;
        }
        current = dir.parent();
    }

    // 3. Walk upward looking for package manifests (package.json, Cargo.toml)
    current = Some(cwd.as_path());
    while let Some(dir) = current {
        let is_home = home.as_deref() == Some(dir);
        let pkg_json = dir.join("package.json");
        if pkg_json.exists() {
            let mut name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_owned();
            if let Ok(content) = fs::read_to_string(&pkg_json) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(n) = val.get("name").and_then(|v| v.as_str()) {
                        name = n.to_owned();
                    }
                }
            }
            return Ok(ProjectCandidate {
                key: slug_key(&name),
                display_name: format_title(&name),
                root: dir.to_path_buf(),
                method: ProjectDetectionMethod::Manifest,
                confidence: 0.82,
            });
        }
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let mut name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_owned();
            if let Ok(content) = fs::read_to_string(&cargo_toml) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("name =") {
                        let parsed = rest.trim().trim_matches('"').trim_matches('\'');
                        if !parsed.is_empty() {
                            name = parsed.to_owned();
                            break;
                        }
                    }
                }
            }
            return Ok(ProjectCandidate {
                key: slug_key(&name),
                display_name: format_title(&name),
                root: dir.to_path_buf(),
                method: ProjectDetectionMethod::Manifest,
                confidence: 0.82,
            });
        }
        if is_home {
            break;
        }
        current = dir.parent();
    }

    // 4. Check if cwd is home directory
    if home.as_deref() == Some(&cwd) {
        return Ok(ProjectCandidate {
            key: "personal-unassigned".into(),
            display_name: "Personal Unassigned".into(),
            root: cwd,
            method: ProjectDetectionMethod::PersonalInbox,
            confidence: 0.4,
        });
    }

    // 5. Cwd fallback
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("personal-unassigned");
    Ok(ProjectCandidate {
        key: slug_key(dir_name),
        display_name: format_title(dir_name),
        root: cwd,
        method: ProjectDetectionMethod::Cwd,
        confidence: 0.6,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn orders_override_above_config_and_git() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(
            root.join(".tokentree.yml"),
            "project: configured\ntitle: Configured Project",
        )
        .unwrap();

        let over = resolve_project(ResolveProjectInput {
            cwd: root.to_path_buf(),
            override_key: Some("manual".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(over.method, ProjectDetectionMethod::Override);
        assert_eq!(over.key, "manual");

        let conf = resolve_project(ResolveProjectInput {
            cwd: root.to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(conf.method, ProjectDetectionMethod::Config);
        assert_eq!(conf.key, "configured");
        assert_eq!(conf.display_name, "Configured Project");
    }

    #[test]
    fn detects_no_git_project_from_manifest() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("package.json"), r#"{"name":"space-game"}"#).unwrap();

        let res = resolve_project(ResolveProjectInput {
            cwd: root.to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(res.method, ProjectDetectionMethod::Manifest);
        assert_eq!(res.key, "space-game");
    }

    #[test]
    fn rejects_forbidden_capabilities_in_config() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join(".tokentree.yml"),
            "project: test\ncommand: rm -rf /\n",
        )
        .unwrap();

        let err = resolve_project(ResolveProjectInput {
            cwd: root.to_path_buf(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("forbidden"));
    }
}
