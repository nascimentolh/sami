//! Qual forja atende o repositório e por onde se fala com ela: GitLab pelo `glab`, GitHub pelo `gh`.
//! Sem forja (remoto que não é de nenhuma das duas, ou repo sem remoto), quem responde é o git.
//!
//! As duas CLIs já têm o login de quem usa, substituem o projeto no endereço (`:id` no glab,
//! `{owner}/{repo}` no gh) e sabem paginar — o que muda é o endereço e o formato da resposta,
//! e isso mora em quem chama.

use std::{path::Path, process::Command};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Forja {
    #[default]
    GitLab,
    GitHub,
}

impl Forja {
    /// Pelo remoto `origin`. Instância própria (gitlab.empresa.com) cai no nome no domínio;
    /// quem não casa com nenhuma das duas fica sem forja, e o Maestro mostra o que o git sabe.
    pub fn de(root: &Path) -> Option<Self> {
        Self::do_remoto(&remoto(root))
    }

    fn do_remoto(url: &str) -> Option<Self> {
        let url = url.to_lowercase();
        match (url.contains("gitlab"), url.contains("github")) {
            (true, _) => Some(Self::GitLab),
            (_, true) => Some(Self::GitHub),
            _ => None,
        }
    }

    pub fn cli(self) -> &'static str {
        match self {
            Self::GitLab => "glab",
            Self::GitHub => "gh",
        }
    }

    pub fn nome(self) -> &'static str {
        match self {
            Self::GitLab => "GitLab",
            Self::GitHub => "GitHub",
        }
    }

    /// Como se chama lá o que aqui é um MR, e o prefixo do número (`!7`, `#7`).
    pub fn termo(self) -> &'static str {
        match self {
            Self::GitLab => "MR",
            Self::GitHub => "PR",
        }
    }

    pub fn marca(self) -> char {
        match self {
            Self::GitLab => '!',
            Self::GitHub => '#',
        }
    }

    /// O que dizer para quem ainda não logou.
    pub fn login(self) -> &'static str {
        match self {
            Self::GitLab => "glab auth login",
            Self::GitHub => "gh auth login",
        }
    }

    pub fn logado(self) -> bool {
        let quem = match self {
            Self::GitLab => "user",
            Self::GitHub => "user",
        };
        Command::new(self.cli())
            .args(["api", quem])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// GET. `paginate` junta as páginas num array só.
    pub fn read(
        self,
        root: Option<&Path>,
        endpoint: &str,
        paginate: bool,
    ) -> Result<Value, String> {
        let mut c = Command::new(self.cli());
        c.arg("api");
        if paginate {
            c.arg("--paginate");
        }
        if let Some(r) = root {
            c.current_dir(r);
        }
        let out = c
            .arg(endpoint)
            .output()
            .map_err(|e| format!("{}: {e}", self.cli()))?;
        if !out.status.success() {
            return Err(primeira_linha(&out.stderr, self.cli()));
        }
        junta_paginas(&out.stdout)
    }
}

/// `dono/repo` do remoto `origin`, de `git@host:dono/repo.git` ou `https://host/dono/repo`.
/// Sai daqui e não da CLI de propósito: num fork, o `gh` resolve para o repositório-mãe.
pub fn projeto(root: &Path) -> Option<String> {
    caminho_do_remoto(&remoto(root))
}

fn caminho_do_remoto(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    // `ssh://git@host:2222/dono/repo`, `https://host/dono/repo` ou o ssh curto `git@host:dono/repo`
    let (host, caminho) = match url.split_once("://") {
        Some((_, resto)) => resto.split_once('/')?,
        None => url.split_once(':')?,
    };
    // caminho local (`/tmp/repo`, `C:/repos/x`) não é remoto de forja
    if !host.contains('.') {
        return None;
    }
    let caminho = caminho.trim_start_matches('/').trim_end_matches(".git");
    caminho.contains('/').then(|| caminho.to_string())
}

fn remoto(root: &Path) -> String {
    Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn primeira_linha(stderr: &[u8], cli: &str) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(cli)
        .trim()
        .to_string()
}

/// `--paginate` cola os arrays das páginas (`[..][..]`): lidos em sequência e achatados.
pub fn junta_paginas(raw: &[u8]) -> Result<Value, String> {
    let mut all = Vec::new();
    for v in serde_json::Deserializer::from_slice(raw).into_iter::<Value>() {
        match v.map_err(|e| e.to_string())? {
            Value::Array(a) => all.extend(a),
            other => return Ok(other),
        }
    }
    Ok(Value::Array(all))
}

/// O que o git sabe sozinho, para o repo sem forja: a branch e o quanto ela anda longe do upstream.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoGit {
    pub branch: String,
    pub upstream: Option<String>,
    pub a_frente: u32,
    pub atras: u32,
}

pub fn so_git(root: &Path) -> SoGit {
    let git = |args: &[&str]| -> Option<String> {
        let o = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let upstream = git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    let (mut a_frente, mut atras) = (0, 0);
    if upstream.is_some()
        && let Some(c) = git(&["rev-list", "--left-right", "--count", "HEAD...@{u}"])
    {
        let mut n = c.split_whitespace().filter_map(|x| x.parse().ok());
        a_frente = n.next().unwrap_or(0);
        atras = n.next().unwrap_or(0);
    }
    SoGit {
        branch,
        upstream,
        a_frente,
        atras,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descobre_a_forja_pelo_remoto() {
        let de = Forja::do_remoto;
        assert_eq!(de("git@gitlab.com:g/p.git"), Some(Forja::GitLab));
        assert_eq!(de("https://GitHub.com/o/r.git"), Some(Forja::GitHub));
        assert_eq!(de("git@gitlab.empresa.com:g/p.git"), Some(Forja::GitLab));
        assert_eq!(de("git@bitbucket.org:g/p.git"), None);
        assert_eq!(de(""), None);
        assert_eq!(Forja::GitHub.termo(), "PR");
        assert_eq!(Forja::GitLab.marca(), '!');
    }

    #[test]
    fn tira_dono_e_repo_do_remoto() {
        let c = |u| caminho_do_remoto(u);
        assert_eq!(
            c("git@github.com:dono/repo.git").as_deref(),
            Some("dono/repo")
        );
        assert_eq!(
            c("https://github.com/dono/repo").as_deref(),
            Some("dono/repo")
        );
        assert_eq!(
            c("git@gitlab.com:grupo/sub/proj.git").as_deref(),
            Some("grupo/sub/proj")
        );
        assert_eq!(
            c("ssh://git@gitlab.com:2222/grupo/proj.git").as_deref(),
            Some("grupo/proj")
        );
        assert_eq!(c("/tmp/repo-local"), None);
    }

    #[test]
    fn junta_as_paginas() {
        let v = junta_paginas(b"[{\"a\":1}][{\"a\":2}]").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(junta_paginas(b"{\"x\":1}").unwrap()["x"], 1);
    }
}
