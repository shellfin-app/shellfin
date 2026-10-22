//! Sidebar snapshot (§7.3): one read-only row per concrete alias (a `Host` pattern without
//! `*`, `?` or `!`), in order of first appearance, with the effective HostName/User/Port,
//! ProxyJump and identity files, the defining `file:line`, and a warning badge.

use std::collections::HashSet;

use crate::ipc::dto::{
    ConfigWarning, ConfigWarningCode, SourceLocation, SshConfigHost, SshConfigSnapshot,
};

use super::parser::{Item, warning};
use super::pattern::is_concrete;
use super::resolve::{ProxyJumpOpt, QueryOverrides, resolve};
use super::validate::valid_hostname;
use super::{ParsedConfig, ParsedFile};

fn collect_aliases(
    file: &ParsedFile,
    seen: &mut HashSet<String>,
    out: &mut Vec<(String, SourceLocation)>,
) {
    for item in &file.items {
        match item {
            Item::Host { patterns, loc } => {
                for p in patterns.iter().filter(|p| is_concrete(p)) {
                    if seen.insert(p.to_lowercase()) {
                        out.push((p.clone(), loc.clone()));
                    }
                }
            }
            Item::Include { files } => {
                for f in files {
                    collect_aliases(f, seen, out);
                }
            }
            Item::Match { .. } | Item::Opt(_) => {}
        }
    }
}

/// Warnings in first-seen order without duplicates, deduplicated through a hash set (/// a linear `contains` per push was O(n²) for a hostile config).
#[derive(Default)]
struct UniqueWarnings {
    seen: HashSet<String>,
    list: Vec<ConfigWarning>,
}

impl UniqueWarnings {
    fn push(&mut self, w: ConfigWarning) {
        // `ConfigWarning` (a DTO) has no `Hash`; its Debug form is an exact, injective key.
        if self.seen.insert(format!("{w:?}")) {
            self.list.push(w);
        }
    }
}

fn push_unique(warnings: &mut UniqueWarnings, w: ConfigWarning) {
    warnings.push(w);
}

impl ParsedConfig {
    /// Concrete aliases in order of first appearance (case-insensitive dedup), with the location
    /// of the first `Host` line naming them.
    pub(crate) fn concrete_aliases(&self) -> Vec<(String, SourceLocation)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for f in [&self.user, &self.system].into_iter().flatten() {
            collect_aliases(f, &mut seen, &mut out);
        }
        out
    }

    /// Whether `alias` is a row of [`Self::snapshot`] (same alias spelling, same listing rule),
    /// without resolving every other host (`bookmark_copy_from_ssh_config`).
    pub fn lists_host(&self, alias: &str) -> bool {
        self.concrete_aliases().iter().any(|(a, _)| a == alias)
            && resolve(self, alias, &self.env, QueryOverrides::default()).is_ok_and(|r| {
                r.config
                    .hostname
                    .as_deref()
                    .map_or_else(|| valid_hostname(alias), valid_hostname)
            })
    }

    /// The UI snapshot. Hosts whose effective HostName (or HostKeyAlias) fails
    /// `valid_hostname` are not listed and produce an `invalidHostname` warning; hosts using
    /// `ProxyCommand` (without ProxyJump) are listed with a `proxyCommandUnsupported` warning.
    pub fn snapshot(&self, loaded_at_ms: i64) -> SshConfigSnapshot {
        let mut warnings = UniqueWarnings::default();
        for w in &self.warnings {
            push_unique(&mut warnings, w.clone());
        }
        let mut hosts = Vec::new();
        for (alias, origin) in self.concrete_aliases() {
            let res = match resolve(self, &alias, &self.env, QueryOverrides::default()) {
                Ok(r) => r,
                Err(e) => {
                    let super::QueryError::InvalidHostname { value } = e.error;
                    push_unique(
                        &mut warnings,
                        warning(e.loc, ConfigWarningCode::InvalidHostname { value }),
                    );
                    continue;
                }
            };
            let cfg = res.config;
            let hostname = cfg.hostname.clone().unwrap_or_else(|| alias.clone());
            if !valid_hostname(&hostname) {
                push_unique(
                    &mut warnings,
                    warning(
                        origin.clone(),
                        ConfigWarningCode::InvalidHostname { value: hostname },
                    ),
                );
                continue;
            }
            let mut has_warnings = !cfg.warnings.is_empty();
            for w in cfg.warnings {
                push_unique(&mut warnings, w);
            }
            if let Some(loc) = res.proxy_command_loc {
                has_warnings = true;
                push_unique(
                    &mut warnings,
                    warning(
                        loc,
                        ConfigWarningCode::ProxyCommandUnsupported {
                            alias: alias.clone(),
                        },
                    ),
                );
            }
            let proxy_jump = match cfg.proxy_jump {
                Some(ProxyJumpOpt::Hops(hops)) if !hops.is_empty() => Some(hops.join(",")),
                _ => None,
            };
            hosts.push(SshConfigHost {
                alias,
                hostname,
                user: cfg.user,
                port: cfg.port.unwrap_or(22),
                proxy_jump,
                identity_files: cfg.identity_files.into_iter().map(|i| i.path).collect(),
                origin,
                has_warnings,
            });
        }
        SshConfigSnapshot {
            main_file: self.env.user_config.display().to_string(),
            files: self
                .files_read
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            hosts,
            warnings: warnings.list,
            dropped_warnings: self.dropped_warnings,
            loaded_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::sshconfig::testutil::Fixture;
    use crate::sshconfig::{SshConfigCache, StdFsSource};
    use pretty_assertions::assert_eq;

    #[test]
    fn alias_enumeration_with_file_and_line() {
        let f = Fixture::new();
        let inc = f.user_file(
            "conf.d/work.conf",
            "# work\nHost work1 work2\n  HostName %h.corp.example.com\n",
        );
        let cfg = f.user_config(
            "Include conf.d/*.conf\nHost *\n  User alice\n\nHost nas Nas web?\n  HostName nas.local\n  Port 2222\n\
             Host !secret bastion\n  ProxyJump none\nHost viajump\n  ProxyJump bastion\n",
        );
        let s = f.load().snapshot(42);
        assert_eq!(s.main_file, cfg.display().to_string());
        assert_eq!(
            s.files,
            vec![cfg.display().to_string(), inc.display().to_string()]
        );
        assert_eq!(s.loaded_at_ms, 42);
        let rows: Vec<(String, String, u16, String, u32)> = s
            .hosts
            .iter()
            .map(|h| {
                (
                    h.alias.clone(),
                    h.hostname.clone(),
                    h.port,
                    h.origin.file.clone(),
                    h.origin.line,
                )
            })
            .collect();
        let c = cfg.display().to_string();
        let i = inc.display().to_string();
        assert_eq!(
            rows,
            vec![
                (
                    "work1".into(),
                    "work1.corp.example.com".into(),
                    22,
                    i.clone(),
                    2
                ),
                ("work2".into(), "work2.corp.example.com".into(), 22, i, 2),
                ("nas".into(), "nas.local".into(), 2222, c.clone(), 5),
                ("bastion".into(), "bastion".into(), 22, c.clone(), 8),
                ("viajump".into(), "viajump".into(), 22, c, 10),
            ]
        );
        assert!(s.hosts.iter().all(|h| h.user.as_deref() == Some("alice")));
        assert_eq!(s.hosts[3].proxy_jump, None);
        assert_eq!(s.hosts[4].proxy_jump.as_deref(), Some("bastion"));
        assert!(s.hosts.iter().all(|h| !h.has_warnings));
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn invalid_hostnames_are_warned_and_not_listed() {
        let f = Fixture::new();
        f.user_config(
            "Host good\n  HostName good.example.com\nHost bad\n  HostName evil/../x\n\
             Host badalias\n  HostKeyAlias %x\nHost a/b\n",
        );
        let s = f.load().snapshot(0);
        assert_eq!(
            s.hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(),
            vec!["good"]
        );
        let codes: Vec<_> = s
            .warnings
            .iter()
            .map(|w| (w.kind.clone(), w.location.line))
            .collect();
        assert_eq!(
            codes,
            vec![
                (
                    ConfigWarningCode::InvalidHostname {
                        value: "evil/../x".into()
                    },
                    4
                ),
                (ConfigWarningCode::InvalidHostname { value: "%x".into() }, 6),
                (
                    ConfigWarningCode::InvalidHostname {
                        value: "a/b".into()
                    },
                    7
                ),
            ]
        );
    }

    #[test]
    fn proxy_command_and_match_warnings_mark_hosts() {
        let f = Fixture::new();
        f.user_config(
            "Host pc\n  ProxyCommand nc %h %p\nHost plain\nMatch host plain exec \"x\"\n  Port 1\n",
        );
        let s = f.load().snapshot(0);
        let pc = &s.hosts[0];
        assert!(pc.has_warnings);
        assert!(
            s.hosts[1].has_warnings,
            "an unsupported Match block that targets the host marks it"
        );
        assert!(
            s.warnings.iter().any(
                |w| w.kind == ConfigWarningCode::ProxyCommandUnsupported { alias: "pc".into() }
            )
        );
        let match_warnings = s
            .warnings
            .iter()
            .filter(|w| matches!(w.kind, ConfigWarningCode::UnsupportedMatchCriteria { .. }))
            .count();
        assert_eq!(
            match_warnings, 1,
            "load and per-host warnings are deduplicated"
        );
    }

    #[test]
    fn counts_and_cache() {
        let f = Fixture::new();
        f.user_file("a.conf", "Host a\n");
        f.user_config("Host x y *.z\nInclude a.conf\n");
        let p = f.load();
        assert_eq!(p.host_count(), 3);
        assert_eq!(p.include_count(), 1);

        let cache = SshConfigCache::default();
        assert!(cache.get().is_none());
        assert!(cache.replace(crate::sshconfig::load(&StdFsSource, &f.env)));
        assert!(
            !cache.replace(crate::sshconfig::load(&StdFsSource, &f.env)),
            "unchanged reload"
        );
        f.user_config("Host x\n");
        assert!(cache.replace(crate::sshconfig::load(&StdFsSource, &f.env)));
        assert_eq!(cache.get().unwrap().host_count(), 1);
    }

    /// `lists_host` agrees with the snapshot rows without building the snapshot.
    #[test]
    fn lists_host_matches_the_snapshot() {
        let f = Fixture::new();
        f.user_config(
            "Host good Good2 *.z !neg\n  HostName good.example.com\nHost badname\n  HostName \"a b\"\n\
             Host tok\n  HostName %Z.example.com\nHost plain\n",
        );
        let p = f.load();
        let listed: Vec<String> = p.snapshot(0).hosts.into_iter().map(|h| h.alias).collect();
        for probe in [
            "good", "Good2", "good2", "GOOD", "*.z", "neg", "!neg", "badname", "tok", "plain",
            "absent", "",
        ] {
            assert_eq!(
                p.lists_host(probe),
                listed.iter().any(|a| a == probe),
                "{probe}: {listed:?}"
            );
        }
        assert!(p.lists_host("good") && p.lists_host("plain"));
    }

    /// A warning flood across the 256-file cap stays bounded and the snapshot
    /// (with hash-set deduplication) is fast.
    #[test]
    fn warning_flood_across_many_files_is_bounded() {
        let f = Fixture::new();
        let junk = "junk\n".repeat(200);
        for i in 0..300 {
            f.user_file(&format!("flood/f{i:03}.conf"), &junk);
        }
        f.user_config("Include flood/*.conf\nHost a\n");
        let start = std::time::Instant::now();
        let p = f.load();
        let s = p.snapshot(0);
        let elapsed = start.elapsed();
        let bad = s
            .warnings
            .iter()
            .filter(|w| matches!(w.kind, ConfigWarningCode::BadLine { .. }))
            .count();
        assert_eq!(
            bad,
            (crate::sshconfig::MAX_CONFIG_FILES - 1)
                * crate::sshconfig::parser::MAX_BAD_LINES_PER_FILE
        );
        // 255 included files of 200 junk lines each, 50 reported per file.
        assert_eq!(
            s.dropped_warnings,
            u32::try_from(255 * (200 - crate::sshconfig::parser::MAX_BAD_LINES_PER_FILE)).unwrap()
        );
        assert_eq!(s.hosts.len(), 1);
        assert!(elapsed < std::time::Duration::from_secs(20), "{elapsed:?}");
    }

    #[test]
    fn empty_when_no_config() {
        let f = Fixture::new();
        let s = f.load().snapshot(1);
        assert!(s.hosts.is_empty() && s.files.is_empty() && s.warnings.is_empty());
        assert_eq!(s.dropped_warnings, 0);
    }
}
