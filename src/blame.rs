//! Blame da linha do cursor e histórico do arquivo, direto do git.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::diff::{self, FileDiff};

#[derive(Debug, Clone, PartialEq)]
pub struct Blame {
    /// vazio quando a linha ainda não foi commitada
    pub sha: String,
    pub author: String,
    pub time: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub time: u64,
    pub subject: String,
    /// `!123` do "See merge request …!123" no corpo, quando houver
    pub mr: Option<u64>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn ago(secs: u64) -> String {
    let m = secs / 60;
    let (h, d) = (m / 60, m / 1440);
    match () {
        _ if m < 1 => "agora".into(),
        _ if m < 60 => format!("há {m} min"),
        _ if h < 24 => format!("há {h} h"),
        _ if d < 14 => format!("há {d} dia{}", if d > 1 { "s" } else { "" }),
        _ if d < 60 => format!("há {} semanas", d / 7),
        _ if d < 365 => format!("há {} meses", d / 30),
        _ => {
            let y = d / 365;
            format!("há {y} ano{}", if y > 1 { "s" } else { "" })
        }
    }
}

pub fn parse_porcelain(out: &str) -> Option<Blame> {
    let mut lines = out.lines();
    let sha = lines.next()?.split(' ').next()?.to_string();
    let field = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(k))
            .unwrap_or_default()
            .to_string()
    };
    let uncommitted = sha.chars().all(|c| c == '0');
    Some(Blame {
        sha: if uncommitted { String::new() } else { sha },
        author: field("author "),
        time: field("author-time ").parse().unwrap_or_default(),
        summary: field("summary "),
    })
}

/// A linha `line` (1-based) do buffer atual: linhas editadas e não salvas saem como não commitadas.
pub fn line(root: &Path, path: &str, line: usize, contents: &str) -> Option<Blame> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["blame", "--porcelain", "-L"])
        .arg(format!("{line},{line}"))
        .args(["--contents", "-", "--", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(contents.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| parse_porcelain(&String::from_utf8_lossy(&out.stdout)))?
}

const SEP: char = '\u{1f}';

pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\u{1e}')
        .filter_map(|c| {
            let mut f = c.trim_matches('\n').split(SEP);
            let sha = f.next()?.to_string();
            if sha.is_empty() {
                return None;
            }
            let author = f.next()?.to_string();
            let time = f.next()?.parse().unwrap_or_default();
            let subject = f.next()?.to_string();
            let body = f.next().unwrap_or_default();
            let mr = body
                .lines()
                .chain([subject.as_str()])
                .find_map(|l| l.split("merge request ").nth(1))
                .and_then(|r| r.rsplit('!').next())
                .and_then(|n| n.trim().parse().ok());
            Some(Commit {
                sha,
                author,
                time,
                subject,
                mr,
            })
        })
        .collect()
}

pub fn history(root: &Path, path: &str) -> Vec<Commit> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            "--follow",
            "-n",
            "200",
            "--format=%h\u{1f}%an\u{1f}%at\u{1f}%s\u{1f}%b\u{1e}",
            "--",
            path,
        ])
        .output();
    out.map(|o| parse_log(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// O que o commit mudou neste arquivo.
pub fn commit_diff(root: &Path, sha: &str, path: &str) -> Vec<FileDiff> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", "--format=", sha, "--", path])
        .output();
    out.map(|o| diff::parse(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_e_log() {
        let p = "51bb0d2aa 3 3 1\nauthor Ana Souza\nauthor-time 1700000000\nsummary fix: timeout do slack\n\tconst x = 1\n";
        let b = parse_porcelain(p).unwrap();
        assert_eq!(
            (
                b.sha.as_str(),
                b.author.as_str(),
                b.time,
                b.summary.as_str()
            ),
            (
                "51bb0d2aa",
                "Ana Souza",
                1700000000,
                "fix: timeout do slack"
            )
        );
        let novo = parse_porcelain("0000000000 1 1 1\nauthor Not Committed Yet\n").unwrap();
        assert!(novo.sha.is_empty());
        let log = "a1\u{1f}Ana\u{1f}10\u{1f}Merge branch 'x' into 'main'\u{1f}feat\n\nSee merge request ctbz/experts/bots-api!147\u{1e}\nb2\u{1f}Bruno\u{1f}5\u{1f}chore: deps\u{1f}\u{1e}\n";
        let c = parse_log(log);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].mr, c[1].mr), (Some(147), None));
        assert_eq!(c[1].subject, "chore: deps");
    }

    #[test]
    fn quanto_tempo() {
        assert_eq!(ago(30), "agora");
        assert_eq!(ago(3 * 3600), "há 3 h");
        assert_eq!(ago(3 * 86400), "há 3 dias");
        assert_eq!(ago(20 * 86400), "há 2 semanas");
        assert_eq!(ago(90 * 86400), "há 3 meses");
    }
}
