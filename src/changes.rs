use std::{path::Path, process::Command};

/// Um arquivo em "Mudanças": tudo que a branch mudou desde a base, commitado ou não.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub new: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    pub base: String,
    pub files: Vec<Change>,
    pub uncommitted: usize,
}

/// A aba "Arquivos": o que o git conhece na worktree, sem os ignorados.
pub fn files(root: &Path) -> Vec<String> {
    git(
        root,
        &["ls-files", "--cached", "--others", "--exclude-standard"],
    )
    .map(|o| o.lines().map(String::from).collect())
    .unwrap_or_default()
}

#[derive(Default)]
struct Dir {
    dirs: std::collections::BTreeMap<String, Dir>,
    files: Vec<String>,
}

/// As linhas visíveis da árvore do repo: (profundidade, nome, caminho, é pasta).
/// Pastas antes dos arquivos; só desce nas pastas de `open`.
pub fn tree(
    files: &[String],
    open: &std::collections::HashSet<String>,
) -> Vec<(usize, String, String, bool)> {
    let mut root = Dir::default();
    for f in files {
        let mut d = &mut root;
        let mut parts = f.split('/').peekable();
        while let Some(p) = parts.next() {
            if parts.peek().is_none() {
                d.files.push(p.to_string());
            } else {
                d = d.dirs.entry(p.to_string()).or_default();
            }
        }
    }
    fn walk(
        d: &Dir,
        prefix: &str,
        depth: usize,
        open: &std::collections::HashSet<String>,
        out: &mut Vec<(usize, String, String, bool)>,
    ) {
        for (name, sub) in &d.dirs {
            let path = format!("{prefix}{name}");
            out.push((depth, name.clone(), path.clone(), true));
            if open.contains(&path) {
                walk(sub, &format!("{path}/"), depth + 1, open, out);
            }
        }
        let mut files = d.files.clone();
        files.sort();
        for f in files {
            out.push((depth, f.clone(), format!("{prefix}{f}"), false));
        }
    }
    let mut out = Vec::new();
    walk(&root, "", 0, open, &mut out);
    out
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    // leitura de fundo não pega o index.lock: um rebase ou commit no terminal ao mesmo tempo não quebra
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn changes(root: &Path) -> Option<Changes> {
    let base = crate::diff::base(root)?;
    let fork = git(root, &["merge-base", "HEAD", &base])?;
    let mut files = parse_numstat(&git(root, &["diff", "--numstat", fork.trim()])?);
    let created = git(
        root,
        &["diff", "--name-only", "--diff-filter=A", fork.trim()],
    )?;
    for f in &mut files {
        f.new = created.lines().any(|l| l == f.path);
    }
    for path in git(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        let lines = std::fs::read_to_string(root.join(path)).map_or(0, |t| t.lines().count());
        files.push(Change {
            path: path.into(),
            added: lines as u32,
            removed: 0,
            new: true,
        });
    }
    let uncommitted = git(root, &["status", "--porcelain"])?.lines().count();
    Some(Changes {
        base,
        files,
        uncommitted,
    })
}

/// `12\t3\tsrc/a.rs`; binário vem como `-\t-`.
fn parse_numstat(text: &str) -> Vec<Change> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let added = it.next()?.parse().unwrap_or(0);
            let removed = it.next()?.parse().unwrap_or(0);
            Some(Change {
                path: it.next()?.into(),
                added,
                removed,
                new: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_o_numstat() {
        let c = parse_numstat("8\t0\tsrc/health.ts\n2\t1\tsrc/app.ts\n-\t-\timg.png\n");
        assert_eq!(c.len(), 3);
        assert_eq!(
            (c[0].added, c[0].removed, c[0].path.as_str()),
            (8, 0, "src/health.ts")
        );
        assert_eq!((c[2].added, c[2].removed), (0, 0));
    }

    #[test]
    fn tree_opens_only_expanded_dirs() {
        let files: Vec<String> = [
            "src/b.rs",
            "README.md",
            "src/a/x.rs",
            "Cargo.toml",
            "src/main.rs",
        ]
        .map(String::from)
        .to_vec();
        let rows = |open: &[&str]| {
            super::tree(&files, &open.iter().map(|s| s.to_string()).collect())
                .into_iter()
                .map(|(d, n, _, dir)| format!("{d}{}{n}", if dir { "/" } else { " " }))
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&[]), ["0/src", "0 Cargo.toml", "0 README.md"]);
        assert_eq!(
            rows(&["src", "src/a"]),
            [
                "0/src",
                "1/a",
                "2 x.rs",
                "1 b.rs",
                "1 main.rs",
                "0 Cargo.toml",
                "0 README.md"
            ]
        );
    }
}
