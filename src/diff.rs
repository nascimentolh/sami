use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// `@@ -1,6 +1,8 @@ …`
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub new: bool,
    /// cabeçalho do git (`diff --git`, `new file mode`, `---`, `+++`) para montar o patch de um bloco
    pub head: String,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn counts(&self) -> (usize, usize) {
        let all = self.hunks.iter().flat_map(|h| &h.lines);
        let added = all.clone().filter(|l| l.kind == Kind::Add).count();
        (added, all.filter(|l| l.kind == Kind::Del).count())
    }

    /// Patch só com o bloco `i`, para `git apply -R` (desfazer) e `git apply` (refazer).
    pub fn hunk_patch(&self, i: usize) -> String {
        let h = &self.hunks[i];
        let mut p = self.head.clone();
        p.push_str(&h.header);
        p.push('\n');
        for l in &h.lines {
            p.push(match l.kind {
                Kind::Context => ' ',
                Kind::Add => '+',
                Kind::Del => '-',
            });
            p.push_str(&l.text);
            p.push('\n');
        }
        p
    }
}

pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    for raw in text.lines() {
        if raw.starts_with("diff --git ") {
            files.push(FileDiff {
                path: raw.rsplit(" b/").next().unwrap_or_default().to_string(),
                new: false,
                head: format!("{raw}\n"),
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if let Some(h) = raw.strip_prefix("@@ ") {
            let mut nums = h.split(' ').take(2).map(|s| {
                s[1..]
                    .split(',')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0)
            });
            old = nums.next().unwrap_or(0);
            new = nums.next().unwrap_or(0);
            f.hunks.push(Hunk {
                header: raw.into(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(h) = f.hunks.last_mut() else {
            if raw.starts_with("new file mode") {
                f.new = true;
            }
            if let Some(p) = raw.strip_prefix("+++ b/") {
                f.path = p.into();
            }
            f.head.push_str(raw);
            f.head.push('\n');
            continue;
        };
        let (kind, text) = match raw.as_bytes().first() {
            Some(b'+') => (Kind::Add, &raw[1..]),
            Some(b'-') => (Kind::Del, &raw[1..]),
            Some(b' ') => (Kind::Context, &raw[1..]),
            // "\ No newline at end of file" e afins
            _ => continue,
        };
        let line = Line {
            kind,
            old: (kind != Kind::Add).then_some(old),
            new: (kind != Kind::Del).then_some(new),
            text: text.into(),
        };
        if kind != Kind::Add {
            old += 1;
        }
        if kind != Kind::Del {
            new += 1;
        }
        h.lines.push(line);
    }
    files
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

/// Tudo que a worktree mudou desde a base (commitado ou não), mais os arquivos novos ainda não rastreados.
pub fn worktree(root: &Path) -> Option<(String, Vec<FileDiff>)> {
    let base = ["main", "master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "--verify", "--quiet", b]).is_some())?;
    let fork = git(root, &["merge-base", "HEAD", base])?;
    let mut files = parse(&git(
        root,
        &["diff", "--no-color", "--no-ext-diff", fork.trim()],
    )?);
    for path in git(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        // `--no-index` sai com 1 quando há diferença: o diff vem no stdout mesmo assim
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "--no-color", "--no-index", "--", "/dev/null", path])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        files.extend(parse(&String::from_utf8_lossy(&out.stdout)));
    }
    Some((base.into(), files))
}

/// O que mudou entre duas fotos (um turno do Claude).
pub fn between(root: &Path, a: &str, b: &str) -> Vec<FileDiff> {
    parse(&git(root, &["diff", "--no-color", "--no-ext-diff", a, b]).unwrap_or_default())
}

/// `git apply` (ou `-R`) de um patch na worktree; erro com a mensagem do git.
pub fn apply(root: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    use std::io::Write;
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).arg("apply");
    if reverse {
        cmd.arg("-R");
    }
    let mut child = cmd
        .arg("-")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(patch.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// O realce do `hl` do protótipo: comentário, string, palavra-chave e número.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tok {
    Comment,
    Str,
    Keyword,
    Number,
}

const KW: &[&str] = &[
    "import",
    "from",
    "export",
    "class",
    "constructor",
    "private",
    "readonly",
    "async",
    "await",
    "return",
    "if",
    "throw",
    "new",
    "const",
    "let",
    "for",
    "of",
    "function",
    "interface",
    "type",
    "this",
    "void",
    "try",
    "catch",
    "true",
    "false",
    "null",
    "unknown",
    "Infinity",
    "fn",
    "pub",
    "use",
    "mut",
    "impl",
    "struct",
    "enum",
    "match",
];

pub fn highlight(line: &str) -> Vec<(std::ops::Range<usize>, Tok)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    if line.trim_start().starts_with('#') {
        return vec![(0..line.len(), Tok::Comment)];
    }
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            out.push((i..b.len(), Tok::Comment));
            break;
        }
        if c == b'"' || c == b'\'' || c == b'`' {
            let end = b[i + 1..]
                .iter()
                .position(|&x| x == c)
                .map_or(b.len(), |p| i + 2 + p);
            out.push((i..end, Tok::Str));
            i = end;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &line[start..i];
            if word.bytes().all(|x| x.is_ascii_digit()) {
                out.push((start..i, Tok::Number));
            } else if KW.contains(&word) {
                out.push((start..i, Tok::Keyword));
            }
            continue;
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/app.ts b/src/app.ts
index 1..2 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,3 +1,4 @@ export
 import express from 'express'
+import { health } from './health'
 const app = express()
-app.listen(3000)
+app.listen(port)
diff --git a/src/health.ts b/src/health.ts
new file mode 100644
--- /dev/null
+++ b/src/health.ts
@@ -0,0 +1,2 @@
+export const health = 1
+// ok
\\ No newline at end of file
";

    #[test]
    fn le_o_diff() {
        let f = parse(DIFF);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "src/app.ts");
        assert_eq!(f[0].counts(), (2, 1));
        let l = &f[0].hunks[0].lines;
        assert_eq!((l[1].kind, l[1].old, l[1].new), (Kind::Add, None, Some(2)));
        assert_eq!((l[3].kind, l[3].old, l[3].new), (Kind::Del, Some(3), None));
        assert_eq!((l[4].old, l[4].new), (None, Some(4)));
        assert!(f[1].new && f[1].path == "src/health.ts");
        assert_eq!(f[1].hunks[0].lines.len(), 2);
        assert!(
            f[0].hunk_patch(0)
                .starts_with("diff --git a/src/app.ts b/src/app.ts\nindex")
        );
    }

    #[test]
    fn realce() {
        let h = highlight("const x = 'a' // b 42");
        assert_eq!(h[0], (0..5, Tok::Keyword));
        assert_eq!(h[1], (10..13, Tok::Str));
        assert_eq!(h[2], (14..21, Tok::Comment));
    }

    #[test]
    fn desfaz_e_refaz_um_bloco() {
        let dir = std::env::temp_dir().join(format!("maestro-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
        ]);
        run(&["checkout", "-qb", "feat"]);
        std::fs::write(dir.join("a.txt"), "um\n2\n3\n4\n5\n6\n7\n8\n9\ndez\n").unwrap();
        std::fs::write(dir.join("novo.txt"), "x\n").unwrap();
        let (base, files) = worktree(&dir).unwrap();
        assert_eq!(base, "main");
        let a = files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(a.hunks.len(), 2);
        apply(&dir, &a.hunk_patch(1), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "um\n2\n3\n4\n5\n6\n7\n8\n9\n10\n"
        );
        apply(&dir, &a.hunk_patch(1), false).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("a.txt"))
                .unwrap()
                .ends_with("dez\n")
        );
        let novo = files.iter().find(|f| f.path == "novo.txt").unwrap();
        assert!(novo.new);
        apply(&dir, &novo.hunk_patch(0), true).unwrap();
        assert!(!dir.join("novo.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
