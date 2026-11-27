//! Credentials already on this machine (desktop): doctl logins, the flyctl login, the
//! kubeconfig. Read-only; [`discover`] never returns a secret, [`read_secret`] returns one only
//! when the user pressed "Use it" (the actor then copies it into the vault).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_yaml::Value;
use zeroize::Zeroizing;

use crate::types::{Discovered, ProviderKind};

fn doctl_files(home: &Path) -> [PathBuf; 2] {
    [
        home.join(".config/doctl/config.yaml"),
        home.join("Library/Application Support/doctl/config.yaml"),
    ]
}

fn fly_file(home: &Path) -> PathBuf {
    home.join(".fly/config.yml")
}

fn yaml(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&text).ok()
}

/// `~/…` for display.
fn shown(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => p.display().to_string(),
    }
}

fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// `(context, token)` per doctl login in one config: `access-token` is `default`.
fn doctl_tokens(v: &Value) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    if let Some(t) = nonempty(&v["access-token"]) {
        out.push(("default".to_owned(), t));
    }
    if let Some(m) = v["auth-contexts"].as_mapping() {
        for (k, t) in m {
            if let (Some(k), Some(t)) = (k.as_str(), nonempty(t))
                && !out.iter().any(|(c, _)| c == k)
            {
                out.push((k.to_owned(), t));
            }
        }
    }
    out
}

fn kubeconfig(home: &Path, env: Option<&OsStr>) -> PathBuf {
    env.and_then(|e| std::env::split_paths(e).find(|p| !p.as_os_str().is_empty()))
        .unwrap_or_else(|| home.join(".kube/config"))
}

/// Every credential found under `home` (and `$KUBECONFIG`), without secrets.
pub fn discover(home: &Path) -> Vec<(ProviderKind, Discovered)> {
    discover_with(home, std::env::var_os("KUBECONFIG").as_deref())
}

fn discover_with(home: &Path, kubeconfig_env: Option<&OsStr>) -> Vec<(ProviderKind, Discovered)> {
    let mut out: Vec<(ProviderKind, Discovered)> = Vec::new();
    for f in doctl_files(home) {
        let Some(v) = yaml(&f) else { continue };
        for (ctx, _) in doctl_tokens(&v) {
            let id = format!("doctl:{ctx}");
            if out.iter().any(|(_, d)| d.id == id) {
                continue;
            }
            out.push((
                ProviderKind::Digitalocean,
                Discovered {
                    id,
                    label: "doctl login".into(),
                    detail: Some(format!("context \"{ctx}\"")),
                    path: shown(home, &f),
                },
            ));
        }
    }
    let f = fly_file(home);
    if yaml(&f).is_some_and(|v| nonempty(&v["access_token"]).is_some()) {
        out.push((
            ProviderKind::Fly,
            Discovered {
                id: "flyctl".into(),
                label: "flyctl login".into(),
                detail: None,
                path: shown(home, &f),
            },
        ));
    }
    let f = kubeconfig(home, kubeconfig_env);
    if let Some(v) = yaml(&f) {
        let n = v["contexts"].as_sequence().map_or(0, Vec::len);
        if n > 0 {
            let path = shown(home, &f);
            out.push((
                ProviderKind::Kubernetes,
                Discovered {
                    id: "kubeconfig".into(),
                    label: "kubeconfig".into(),
                    detail: Some(format!(
                        "{n} context{} in {path}",
                        if n == 1 { "" } else { "s" }
                    )),
                    path,
                },
            ));
        }
    }
    out
}

/// The token behind a [`Discovered::id`] (`doctl:<context>`, `flyctl`); `None` for an unknown
/// id or one with nothing to copy (`kubeconfig`: kubectl uses it in place).
pub fn read_secret(home: &Path, id: &str) -> Option<Zeroizing<String>> {
    if let Some(ctx) = id.strip_prefix("doctl:") {
        return doctl_files(home).iter().find_map(|f| {
            let v = yaml(f)?;
            doctl_tokens(&v)
                .into_iter()
                .find(|(c, _)| c == ctx)
                .map(|(_, t)| Zeroizing::new(t.to_owned()))
        });
    }
    if id == "flyctl" {
        let v = yaml(&fly_file(home))?;
        return nonempty(&v["access_token"]).map(|t| Zeroizing::new(t.to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn home() -> Home {
        let p = std::env::temp_dir().join(format!("cloudnodes-disc-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&p).unwrap();
        Home(p)
    }

    fn put(h: &Path, rel: &str, text: &str) {
        let p = h.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn finds_logins_without_returning_secrets() {
        let h = home();
        put(&h.0, ".config/doctl/config.yaml", "access-token: dop_v1_default\nauth-contexts:\n  work: dop_v1_work\n  empty: \"\"\ncontext: default\n");
        put(&h.0, "Library/Application Support/doctl/config.yaml", "access-token: dop_v1_other\nauth-contexts:\n  mac: dop_v1_mac\n");
        put(&h.0, ".fly/config.yml", "access_token: fo1_fly\n");
        put(&h.0, ".kube/config", "apiVersion: v1\ncontexts:\n- name: a\n- name: b\ncurrent-context: a\n");
        let d = discover_with(&h.0, None);
        let ids: Vec<&str> = d.iter().map(|(_, d)| d.id.as_str()).collect();
        assert_eq!(ids, ["doctl:default", "doctl:work", "doctl:mac", "flyctl", "kubeconfig"]);
        assert_eq!(d[1].1.detail.as_deref(), Some("context \"work\""));
        assert_eq!(d[1].1.label, "doctl login");
        assert_eq!(d[0].1.path, "~/.config/doctl/config.yaml");
        assert_eq!(d[4].0, ProviderKind::Kubernetes);
        assert_eq!(d[4].1.detail.as_deref(), Some("2 contexts in ~/.kube/config"));
        let all = serde_json::to_string(&d.iter().map(|(_, d)| d).collect::<Vec<_>>()).unwrap();
        assert!(!all.contains("dop_v1") && !all.contains("fo1_"));

        assert_eq!(read_secret(&h.0, "doctl:default").as_deref().map(String::as_str), Some("dop_v1_default"));
        assert_eq!(read_secret(&h.0, "doctl:work").as_deref().map(String::as_str), Some("dop_v1_work"));
        assert_eq!(read_secret(&h.0, "doctl:mac").as_deref().map(String::as_str), Some("dop_v1_mac"));
        assert_eq!(read_secret(&h.0, "flyctl").as_deref().map(String::as_str), Some("fo1_fly"));
        assert!(read_secret(&h.0, "doctl:empty").is_none());
        assert!(read_secret(&h.0, "kubeconfig").is_none());
    }

    #[test]
    fn kubeconfig_env_and_empty_home() {
        let h = home();
        assert!(discover_with(&h.0, None).is_empty());
        put(&h.0, "k/one", "contexts:\n- name: only\n");
        let env = h.0.join("k/one");
        let d = discover_with(&h.0, Some(env.as_os_str()));
        assert_eq!(d[0].1.detail.as_deref(), Some("1 context in ~/k/one"));
    }
}
