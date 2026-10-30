//! The model picker's layout: the CLI's flat model list grouped by family
//! ("Fable", "Opus"…), most capable first, each with its versions newest
//! first, and the `default` entry apart.

use crate::session::ModelInfo;

/// One model family and its versions, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    pub versions: Vec<Version>,
}

/// A version of a family: what to pass to `--model`, and how to show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub value: String,
    /// "5.5"; empty for a model whose name carries no version.
    pub label: String,
    pub description: String,
}

/// The `default` entry, then the families.
pub fn group(models: &[ModelInfo]) -> (Option<ModelInfo>, Vec<Family>) {
    let default = models.iter().find(|m| m.value == "default").cloned();
    let mut families: Vec<Family> = Vec::new();
    for m in models.iter().filter(|m| m.value != "default") {
        let (family, label) = split(&m.name);
        let i = match families.iter().position(|f| f.name == family) {
            Some(i) => i,
            None => {
                families.push(Family { name: family.to_string(), versions: Vec::new() });
                families.len() - 1
            }
        };
        // An alias ("opus") comes before the full name of the same version; keep it.
        let versions = &mut families[i].versions;
        if !versions.iter().any(|v| v.label == label) {
            versions.push(Version {
                value: m.value.clone(),
                label: label.to_string(),
                description: m.description.clone(),
            });
        }
    }
    for f in &mut families {
        f.versions.sort_by_key(|v| std::cmp::Reverse(number(&v.label)));
    }
    families.sort_by_key(|f| POWER.iter().position(|p| p.eq_ignore_ascii_case(&f.name)).unwrap_or(POWER.len()));
    (default, families)
}

/// Families most capable first; others follow in the CLI's order.
const POWER: [&str; 4] = ["Fable", "Opus", "Sonnet", "Haiku"];

/// "Opus 4.8" → ("Opus", "4.8"); a name without a version is all family.
fn split(name: &str) -> (&str, &str) {
    match name.trim().rsplit_once(' ') {
        Some((family, version)) if version.starts_with(|c: char| c.is_ascii_digit()) => (family.trim(), version),
        _ => (name.trim(), ""),
    }
}

/// "4.10" → [4, 10], so versions sort as numbers; anything else sorts last.
fn number(label: &str) -> Vec<u32> {
    label.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(value: &str, name: &str) -> ModelInfo {
        ModelInfo { value: value.into(), name: name.into(), description: format!("{name}!"), efforts: Vec::new() }
    }

    #[test]
    fn groups_by_family_most_capable_and_newest_first() {
        let list = [
            m("default", "Default (recommended)"),
            m("opus", "Opus 5.5"),
            m("fable", "Fable 5.1"),
            m("sonnet", "Sonnet 5.5"),
            m("claude-haiku-4-5", "Haiku 4.5"),
            m("haiku", "Haiku 5.5"),
            m("claude-opus-4-8", "Opus 4.8"),
            m("claude-opus-5", "Opus 5"),
            m("claude-opus-4-10", "Opus 4.10"),
            m("claude-fable-5", "Fable 5"),
            m("mystery", "Mystery 1"),
        ];
        let (default, families) = group(&list);
        assert_eq!(default.unwrap().value, "default");
        let names: Vec<_> = families.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Fable", "Opus", "Sonnet", "Haiku", "Mystery"]);
        let opus: Vec<_> = families[1].versions.iter().map(|v| (v.label.as_str(), v.value.as_str())).collect();
        assert_eq!(
            opus,
            [("5.5", "opus"), ("5", "claude-opus-5"), ("4.10", "claude-opus-4-10"), ("4.8", "claude-opus-4-8")]
        );
        assert_eq!(families[3].versions[0].value, "haiku");
        assert_eq!(families[1].versions[0].description, "Opus 5.5!");
    }

    #[test]
    fn an_alias_wins_over_the_same_version_and_bare_names_stand_alone() {
        let (default, families) = group(&[m("opus", "Opus 5.5"), m("claude-opus-5-5", "Opus 5.5"), m("x", "Opus")]);
        assert!(default.is_none());
        assert_eq!(families.len(), 1);
        let labels: Vec<_> = families[0].versions.iter().map(|v| (v.label.as_str(), v.value.as_str())).collect();
        assert_eq!(labels, [("5.5", "opus"), ("", "x")]);
        assert_eq!(split("  "), ("", ""));
        assert_eq!(split("Claude Opus 5"), ("Claude Opus", "5"));
    }
}
