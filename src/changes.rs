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

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
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
    let base = ["main", "master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "--verify", "--quiet", b]).is_some())?;
    let fork = git(root, &["merge-base", "HEAD", base])?;
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
        base: base.into(),
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
}
