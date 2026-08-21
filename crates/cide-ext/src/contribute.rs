//! Merging what everything contributes into the one set the app actually uses.
//!
//! # The rule, in one line
//!
//! **Builtins first, then extensions in install order; a later claim on the same *name* wins over
//! a builtin and loses to an earlier extension, and every displacement is reported.**
//!
//! # Why an extension beats a builtin
//!
//! Because otherwise installing one would do nothing. cide ships SQL and YAML grammars of its own,
//! so if the builtin won, the two extensions this milestone exists to demonstrate would install
//! successfully, appear in the panel, and change nothing on screen — which is indistinguishable
//! from a broken extension host and is the worst possible first impression of the feature.
//!
//! The displacement is recorded rather than silent ([`ContributionSource`] on every binding, and a
//! `supersedes` beside it) because *"why is my `.sql` file coloured like that"* has to be
//! answerable without reading two manifests.
//!
//! # Why an *earlier* extension beats a later one
//!
//! This is the only rule here that had a real alternative. The plan said grey both and name both
//! manifests, on `cide-agents`' precedent for two files claiming one agent name — where refusing
//! to guess is right, because the two files are in one directory the user is editing and the fix
//! is one rename away.
//!
//! Two *extensions* are not that. They come from different marketplaces, and the user installing
//! the second one has no idea it collides with the first. Dropping both would mean **installing a
//! new extension silently disables a working one**, and the symptom — SQL highlighting stops — is
//! nowhere near the gesture that caused it. First-wins keeps everything that worked before the
//! install working after it, and the loser is reported with both manifests named, which is the
//! half of "grey both" that was actually carrying the value.

use std::collections::{BTreeMap, BTreeSet};

use cide_ipc::ext::{
    ContributionSource, ExtProblem, ExtensionRef, LanguageBinding, PanelBinding,
    ResolvedContributions, ResolvedExtCommand, ServerBinding,
};
use cide_ipc::lang::{LanguageDef, LanguageServerDef};

use crate::manifest::{panel_view, resolved_commands, warning};

/// One extension's contributions, as the merge sees them.
pub struct Contributor<'a> {
    pub id: ExtensionRef,
    /// The manifest this came from, for naming in a conflict.
    pub manifest: std::path::PathBuf,
    pub contributes: &'a cide_ipc::ext::Contributions,
}

/// Merge builtins and enabled extensions.
///
/// `extensions` must already be filtered to the enabled, non-greyed ones: this function has no
/// opinion about whether an extension should be running, only about what happens when two that are
/// both want the same name.
#[must_use]
pub fn resolve(
    builtin_languages: Vec<LanguageDef>,
    builtin_servers: Vec<LanguageServerDef>,
    extensions: &[Contributor<'_>],
) -> ResolvedContributions {
    let mut conflicts: Vec<ExtProblem> = Vec::new();

    // --- languages -----------------------------------------------------------------------
    //
    // Keyed by id, and the *file extension* claims checked separately: two languages with
    // different ids may still both claim `.sql`, and that is the collision a user actually sees.
    let mut by_id: BTreeMap<String, LanguageBinding> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for def in builtin_languages {
        order.push(def.id.clone());
        by_id.insert(
            def.id.clone(),
            LanguageBinding {
                def,
                source: ContributionSource::Builtin,
                supersedes: None,
            },
        );
    }

    // Which extension claimed each file extension and whole-file name, so a second claim can name
    // the first. Seeded from the builtins, which is why an extension claiming `.sql` reports that
    // it displaced the builtin rather than reporting nothing.
    let mut claimed_by: BTreeMap<String, ContributionSource> = BTreeMap::new();
    for id in &order {
        let binding = &by_id[id];
        for ext in &binding.def.extensions {
            claimed_by.insert(format!(".{}", ext.ext), ContributionSource::Builtin);
        }
        for name in &binding.def.filenames {
            claimed_by.insert(name.clone(), ContributionSource::Builtin);
        }
    }

    for contributor in extensions {
        let source = ContributionSource::Extension {
            extension: contributor.id.clone(),
        };
        for def in &contributor.contributes.languages {
            // A claim collides when some *other extension* already holds it. A builtin holding it
            // is not a collision, it is the point.
            let mut blocked: Option<ContributionSource> = None;
            for key in claim_keys(def) {
                match claimed_by.get(&key) {
                    Some(ContributionSource::Extension { extension })
                        if *extension != contributor.id =>
                    {
                        conflicts.push(warning(
                            &contributor.manifest,
                            None,
                            format!(
                                "`{}` claims `{key}`, which `{extension}` already claims. The \
                                 first one installed keeps it — otherwise installing this \
                                 extension would silently stop that one working.",
                                def.id
                            ),
                        ));
                        blocked = Some(ContributionSource::Extension {
                            extension: extension.clone(),
                        });
                    }
                    _ => {}
                }
            }
            if blocked.is_some() {
                continue;
            }

            let supersedes = by_id.get(&def.id).map(|prior| prior.source.clone());
            if !by_id.contains_key(&def.id) {
                order.push(def.id.clone());
            }
            for key in claim_keys(def) {
                claimed_by.insert(key, source.clone());
            }
            by_id.insert(
                def.id.clone(),
                LanguageBinding {
                    def: def.clone(),
                    source: source.clone(),
                    supersedes,
                },
            );
        }
    }

    let languages: Vec<LanguageBinding> = order
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect();
    let known_ids: BTreeSet<&str> = languages.iter().map(|b| b.def.id.as_str()).collect();

    // --- servers -------------------------------------------------------------------------
    //
    // Keyed by binary, because that is what is actually spawned: two extensions both driving
    // `yaml-language-server` would otherwise start two of it against the same root.
    let mut servers: Vec<ServerBinding> = builtin_servers
        .into_iter()
        .map(|def| ServerBinding {
            def,
            source: ContributionSource::Builtin,
        })
        .collect();
    for contributor in extensions {
        let source = ContributionSource::Extension {
            extension: contributor.id.clone(),
        };
        for def in &contributor.contributes.language_servers {
            if let Some(prior) = servers.iter().find(|s| s.def.binary == def.binary) {
                conflicts.push(warning(
                    &contributor.manifest,
                    None,
                    format!(
                        "`{}` is already driven by {}. The first one wins; two of the same \
                         server against one project is two answers to every question.",
                        def.binary,
                        describe(&prior.source)
                    ),
                ));
                continue;
            }
            // A server for a language nothing contributes would start and be sent nothing. Warned
            // rather than dropped: the language may arrive when another extension is enabled, and
            // an install order that changes behaviour is worse than a note.
            for id in &def.language_ids {
                if !known_ids.contains(id.as_str()) {
                    conflicts.push(warning(
                        &contributor.manifest,
                        None,
                        format!(
                            "`{}` claims language `{id}`, which nothing contributes. It will not \
                             be started.",
                            def.binary
                        ),
                    ));
                }
            }
            servers.push(ServerBinding {
                def: def.clone(),
                source: source.clone(),
            });
        }
    }

    // --- panels and commands -------------------------------------------------------------
    //
    // No collision is possible: both are namespaced by the extension that declared them, and the
    // manifest reader has already refused a duplicate inside one extension. That is worth saying
    // out loud, because it is the reason this half is four lines and the languages above are
    // eighty — a namespace bought it.
    let mut panels: Vec<PanelBinding> = Vec::new();
    let mut commands: Vec<ResolvedExtCommand> = Vec::new();
    for contributor in extensions {
        for def in &contributor.contributes.panels {
            panels.push(PanelBinding {
                view: panel_view(
                    &contributor.id.marketplace,
                    &contributor.id.extension,
                    &def.id,
                ),
                def: def.clone(),
                extension: contributor.id.clone(),
            });
        }
        commands.extend(resolved_commands(
            &contributor.id.marketplace,
            &contributor.id.extension,
            contributor.contributes,
        ));
    }

    ResolvedContributions {
        languages,
        servers,
        panels,
        commands,
        conflicts,
    }
}

/// Everything a language definition claims, as lookup keys.
///
/// A file extension is prefixed with a `.` and a whole-file name is not, so `md` the extension and
/// a file literally called `md` cannot collide in this map. Fence aliases are deliberately absent:
/// two languages both answering to ` ```console ` is a cosmetic tie in a markdown preview, and
/// refusing an install over it would be out of proportion.
fn claim_keys(def: &LanguageDef) -> Vec<String> {
    let mut keys: Vec<String> = def
        .extensions
        .iter()
        .map(|e| format!(".{}", e.ext.to_ascii_lowercase()))
        .collect();
    keys.extend(def.filenames.iter().map(|n| n.to_ascii_lowercase()));
    keys
}

fn describe(source: &ContributionSource) -> String {
    match source {
        ContributionSource::Builtin => "cide itself".to_string(),
        ContributionSource::Extension { extension } => format!("`{extension}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cide_ipc::ext::Contributions;
    use cide_ipc::ids::{ExtensionId, MarketplaceId};
    use cide_ipc::lang::{FileExtension, GrammarSpecDto};

    fn language(id: &str, ext: &str) -> LanguageDef {
        LanguageDef {
            id: id.into(),
            label: id.into(),
            extensions: vec![FileExtension {
                ext: ext.into(),
                label: None,
            }],
            filenames: vec![],
            fence_aliases: vec![],
            grammar: GrammarSpecDto {
                name: id.into(),
                ..GrammarSpecDto::default()
            },
            fold: Default::default(),
            scratch: vec![],
        }
    }

    fn reference(market: &str, ext: &str) -> ExtensionRef {
        ExtensionRef {
            marketplace: MarketplaceId(market.into()),
            extension: ExtensionId(ext.into()),
        }
    }

    #[test]
    fn an_extension_supersedes_a_builtin_and_says_so() {
        let contributes = Contributions {
            languages: vec![language("sql", "sql")],
            ..Contributions::default()
        };
        let id = reference("m", "sql");
        let resolved = resolve(
            vec![language("sql", "sql")],
            vec![],
            &[Contributor {
                id: id.clone(),
                manifest: "/m/sql/cide-extension.json".into(),
                contributes: &contributes,
            }],
        );
        let binding = resolved
            .languages
            .iter()
            .find(|b| b.def.id == "sql")
            .expect("sql");
        assert_eq!(
            binding.source,
            ContributionSource::Extension { extension: id },
            "installing an extension that changes nothing on screen is indistinguishable from a \
             broken extension host"
        );
        assert_eq!(binding.supersedes, Some(ContributionSource::Builtin));
        assert!(
            resolved.conflicts.is_empty(),
            "a builtin losing is not a conflict"
        );
    }

    #[test]
    fn the_first_extension_to_claim_an_extension_keeps_it() {
        let first = Contributions {
            languages: vec![language("sql", "sql")],
            ..Contributions::default()
        };
        let second = Contributions {
            languages: vec![language("tsql", "sql")],
            ..Contributions::default()
        };
        let a = reference("m", "sql");
        let b = reference("m", "tsql");
        let resolved = resolve(
            vec![],
            vec![],
            &[
                Contributor {
                    id: a.clone(),
                    manifest: "/m/sql/cide-extension.json".into(),
                    contributes: &first,
                },
                Contributor {
                    id: b,
                    manifest: "/m/tsql/cide-extension.json".into(),
                    contributes: &second,
                },
            ],
        );
        let ids: Vec<&str> = resolved
            .languages
            .iter()
            .map(|l| l.def.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["sql"],
            "installing a new extension must not silently stop a working one"
        );
        assert_eq!(resolved.conflicts.len(), 1);
        assert!(resolved.conflicts[0].message.contains("m.sql"));
    }

    #[test]
    fn two_extensions_cannot_start_the_same_server_twice() {
        let server = |binary: &str| LanguageServerDef {
            binary: binary.into(),
            args: vec![],
            language_ids: vec!["yaml".into()],
            project_markers: vec![],
            project_kind: "any".into(),
            install_hint: "npm i -g yaml-language-server".into(),
            declares_watched_files: false,
            extra_path_hints: vec![],
        };
        let first = Contributions {
            languages: vec![language("yaml", "yaml")],
            language_servers: vec![server("yaml-language-server")],
            ..Contributions::default()
        };
        let second = Contributions {
            language_servers: vec![server("yaml-language-server")],
            ..Contributions::default()
        };
        let resolved = resolve(
            vec![],
            vec![],
            &[
                Contributor {
                    id: reference("m", "yaml"),
                    manifest: "/m/yaml/cide-extension.json".into(),
                    contributes: &first,
                },
                Contributor {
                    id: reference("m", "yaml2"),
                    manifest: "/m/yaml2/cide-extension.json".into(),
                    contributes: &second,
                },
            ],
        );
        assert_eq!(resolved.servers.len(), 1);
        assert!(
            resolved
                .conflicts
                .iter()
                .any(|c| c.message.contains("two answers"))
        );
    }
}
