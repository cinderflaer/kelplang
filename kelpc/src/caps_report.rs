//! `kelpc caps` and the `[dependencies] caps` build check.
//!
//! The report has one line per project module: what the module holds (the
//! grant its importers gave it), what it reaches (the gated items it names,
//! recorded by the capability gate in `infer.rs`), and what it grants to the
//! modules it imports. Every list is sorted, so the output is the same for the
//! same sources and can be compared in CI. Each dependency gets one more line
//! after the modules: the domains its modules reach, and the caps its manifest
//! entry allows.
//!
//! The check refuses a dependency whose modules reach a domain its
//! `coral.toml` entry does not list. A plain-string entry carries no list and
//! so no extra check. Enforcement itself is untouched: a module without a grant
//! still cannot name a gated item, and this check only refuses a build that the
//! manifest already disallows.

use std::collections::{BTreeMap, BTreeSet};

use crate::diag::{Diagnostic, SourceMap};
use crate::infer::CapUse;
use crate::resolve::Project;

/// The dependency a module belongs to: Coral copies dependency `<key>` into
/// `src/deps/<key>/`, so its modules are `deps/<key>/...`.
fn dependency_of(module: &str) -> Option<&str> {
    let rest = module.strip_prefix("deps/")?;
    let (key, _) = rest.split_once('/')?;
    Some(key)
}

/// The standard library's modules are not the project's, so the report and the
/// check leave them out.
fn is_std(module: &str) -> bool {
    module.starts_with("std/")
}

fn quoted_list(patterns: &[String]) -> String {
    let items: Vec<String> = patterns.iter().map(|p| format!("\"{p}\"")).collect();
    format!("[{}]", items.join(", "))
}

/// The domains a module reaches, sorted and without repeats.
fn reached(uses: &[CapUse], keep: impl Fn(&CapUse) -> bool) -> Vec<String> {
    let set: BTreeSet<&str> = uses
        .iter()
        .filter(|u| keep(u))
        .map(|u| u.domain.as_str())
        .collect();
    set.into_iter().map(str::to_string).collect()
}

fn module_line(project: &Project, uses: &[CapUse], module: &str) -> String {
    let held: BTreeMap<String, Vec<String>> = project.caps_of(module).into_iter().collect();
    let holds: Vec<String> = held
        .iter()
        .filter(|(_, patterns)| !patterns.is_empty())
        .map(|(domain, patterns)| format!("{domain}={}", quoted_list(patterns)))
        .collect();
    let holds = if holds.is_empty() {
        "nothing".to_string()
    } else {
        holds.join(", ")
    };

    let used = reached(uses, |u| u.module == module);
    let uses_text = if used.is_empty() {
        "nothing".to_string()
    } else {
        used.join(", ")
    };

    // One group per target module, the domain grants and the wildcard grants
    // of that target together, sorted.
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for grant in project.grants().iter().filter(|g| g.importer == module) {
        groups
            .entry(grant.target.as_str())
            .or_default()
            .push(format!("{}={}", grant.domain, quoted_list(&grant.patterns)));
    }
    for (importer, target) in project.wildcard_grants() {
        if importer == module {
            groups.entry(target.as_str()).or_default().push("*".to_string());
        }
    }
    let grants: Vec<String> = groups
        .into_iter()
        .map(|(target, mut entries)| {
            entries.sort();
            format!("{target}({})", entries.join(", "))
        })
        .collect();
    let grants = if grants.is_empty() {
        "nothing".to_string()
    } else {
        grants.join(", ")
    };

    format!("{module}: holds {holds}; uses {uses_text}; grants {grants}")
}

fn dependency_line(uses: &[CapUse], key: &str, caps: &Option<Vec<String>>) -> String {
    let reached = reached(uses, |u| dependency_of(&u.module) == Some(key));
    let uses_text = if reached.is_empty() {
        "nothing".to_string()
    } else {
        reached.join(", ")
    };
    let allowed = match caps {
        None => "any".to_string(),
        Some(list) if list.is_empty() => "nothing".to_string(),
        Some(list) => {
            let mut sorted = list.clone();
            sorted.sort();
            sorted.join(", ")
        }
    };
    format!("dependency {key}: uses {uses_text}; allowed {allowed}")
}

/// The whole report for a project, one line per line of output, each ending in
/// a newline. Modules come first, sorted by id, then dependencies, sorted by key.
pub fn render(project: &Project, uses: &[CapUse]) -> String {
    let modules: BTreeSet<&str> = project
        .modules
        .iter()
        .map(|m| m.id.as_str())
        .filter(|id| !is_std(id))
        .collect();
    let mut out = String::new();
    for module in modules {
        out.push_str(&module_line(project, uses, module));
        out.push('\n');
    }
    let mut dependencies: Vec<_> = project.dependencies().to_vec();
    dependencies.sort_by(|a, b| a.key.cmp(&b.key));
    for dep in &dependencies {
        out.push_str(&dependency_line(uses, &dep.key, &dep.caps));
        out.push('\n');
    }
    out
}

/// Refuses the first use, in sorted order, of a domain that its dependency's
/// `coral.toml` entry does not list. A dependency without a list is not checked.
pub fn check_dependencies(
    project: &Project,
    uses: &[CapUse],
    sources: &SourceMap,
) -> Result<(), Diagnostic> {
    let limits: BTreeMap<&str, &[String]> = project
        .dependencies()
        .iter()
        .filter_map(|d| d.caps.as_deref().map(|caps| (d.key.as_str(), caps)))
        .collect();
    if limits.is_empty() {
        return Ok(());
    }
    for u in uses {
        if is_std(&u.module) {
            continue;
        }
        let Some(key) = dependency_of(&u.module) else {
            continue;
        };
        let Some(allowed) = limits.get(key) else {
            continue;
        };
        if allowed.iter().any(|c| c == &u.domain) {
            continue;
        }
        let line = sources
            .path_line_col(&u.module, u.span)
            .map(|(_, line, _)| line)
            .unwrap_or(0);
        let listed = allowed
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Diagnostic::new(
            format!(
                "module '{}' uses the '{}' capability at line {line} ('{}'), which dependency '{key}' is not allowed: its coral.toml [dependencies] entry lists caps = [{listed}]",
                u.module, u.domain, u.callee
            ),
            u.span,
            u.module.clone(),
        ));
    }
    Ok(())
}
