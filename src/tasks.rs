//! Tarefas do repo (scripts do package.json, alvos do Makefile, cargo, go) e os erros que elas imprimem.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub name: String,
    pub cmd: String,
}

pub fn detect(root: &Path) -> Vec<Task> {
    let mut out = Vec::new();
    let t = |name: &str, cmd: String| Task {
        name: name.into(),
        cmd,
    };
    if let Some(pkg) = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        let pm = ["pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb"]
            .iter()
            .find(|l| root.join(l).is_file())
            .map_or("npm", |l| l.split(['-', '.']).next().unwrap_or("npm"));
        for name in pkg["scripts"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            let cmd = match (pm, name.as_str()) {
                ("npm", "test" | "start") => format!("npm {name}"),
                ("npm" | "bun", _) => format!("{pm} run {name}"),
                _ => format!("{pm} {name}"),
            };
            out.push(t(name, cmd));
        }
    }
    if let Ok(mk) = std::fs::read_to_string(root.join("Makefile")) {
        for target in mk.lines().filter_map(make_target) {
            out.push(t(&format!("make {target}"), format!("make {target}")));
        }
    }
    if root.join("Cargo.toml").is_file() {
        for c in ["cargo test", "cargo clippy --all-targets", "cargo build"] {
            out.push(t(c, c.into()));
        }
    }
    if root.join("go.mod").is_file() {
        for c in ["go test ./...", "go vet ./...", "go build ./..."] {
            out.push(t(c, c.into()));
        }
    }
    out
}

fn make_target(l: &str) -> Option<&str> {
    let (name, rest) = l.split_once(':')?;
    let ok = !name.is_empty()
        && !rest.starts_with('=')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ok.then_some(name)
}

/// Um erro ou aviso que a tarefa imprimiu, com o arquivo como apareceu na saída.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub file: String,
    pub line: u32,
    pub col: Option<u32>,
    pub msg: String,
    pub warn: bool,
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            it.next();
            for c in it.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "/._-@~".contains(c)
}

fn digits(s: &str) -> (Option<u32>, &str) {
    let n = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    (s[..n].parse().ok(), &s[n..])
}

/// O primeiro `caminho:linha(:coluna)` da linha: (caminho, linha, coluna, o que vem depois).
fn locate(s: &str) -> Option<(&str, u32, Option<u32>, &str)> {
    for (i, _) in s.match_indices(':') {
        let (Some(line), rest) = digits(&s[i + 1..]) else {
            continue;
        };
        let start = s[..i]
            .char_indices()
            .rev()
            .take_while(|(_, c)| path_char(*c))
            .last()
            .map_or(i, |(j, _)| j);
        let path = &s[start..i];
        let name = path.rsplit('/').next().unwrap_or(path);
        if !name.contains('.') || name.ends_with('.') || s[..start].ends_with(':') {
            continue;
        }
        let (col, rest) = match rest.strip_prefix(':') {
            Some(r) => match digits(r) {
                (Some(c), r) => (Some(c), r),
                _ => (None, rest),
            },
            None => (None, rest),
        };
        return Some((path, line, col, rest));
    }
    None
}

/// `src/a.ts(4,7): error TS2322: …`, o formato do tsc sem `--pretty`.
fn tsc_paren(s: &str) -> Option<(&str, u32, Option<u32>, &str)> {
    let (path, rest) = s.split_once('(')?;
    if path.is_empty() || !path.chars().all(path_char) {
        return None;
    }
    let (line, rest) = digits(rest);
    let (col, rest) = digits(rest.strip_prefix(',')?);
    Some((path, line?, col, rest.strip_prefix("):")?))
}

fn severity(rest: &str) -> (bool, &str) {
    let r = rest.trim_start_matches([':', ' ', '-']);
    if let Some(m) = r.strip_prefix("warning") {
        (true, m.trim_start_matches([':', ' ']))
    } else if let Some(m) = r.strip_prefix("error") {
        (false, m.trim_start_matches([':', ' ']))
    } else {
        (false, r)
    }
}

/// Os erros na saída de tsc, eslint, jest, cargo, go e de quem imprime `caminho:linha:coluna`.
pub fn problems(out: &str) -> Vec<Problem> {
    let mut found: Vec<Problem> = Vec::new();
    let mut pending: Option<(bool, String)> = None;
    let mut eslint_file: Option<String> = None;
    let mut jest_title: Option<String> = None;
    for raw in strip_ansi(out).lines() {
        let l = raw.trim_end();
        let t = l.trim_start();
        if let Some(title) = t.strip_prefix("● ") {
            jest_title = Some(title.trim().to_string());
            continue;
        }
        // cargo: `error[E0308]: …` e, linhas depois, ` --> arquivo:linha:coluna`
        if let Some(rest) = l.strip_prefix("error").or(l.strip_prefix("warning")) {
            let warn = l.starts_with("warning");
            let (code, msg) = match rest.strip_prefix('[') {
                Some(r) => r.split_once("]:").unwrap_or(("", r)),
                None => ("", rest.trim_start_matches(':')),
            };
            let msg = msg.trim();
            if !msg.is_empty()
                && !msg.starts_with("could not compile")
                && !msg.contains(" generated ")
            {
                let m = if code.is_empty() {
                    msg.into()
                } else {
                    format!("{code}: {msg}")
                };
                pending = Some((warn, m));
            }
            continue;
        }
        if let Some(arrow) = t.strip_prefix("--> ") {
            if let (Some((warn, msg)), Some((p, line, col, _))) = (pending.take(), locate(arrow)) {
                push(&mut found, p, line, col, msg, warn);
            }
            continue;
        }
        // eslint (stylish): o arquivo sozinho numa linha e, embaixo, `  4:7  error  msg  regra`
        if !l.starts_with(' ') && t.starts_with('/') && !t.contains(' ') && t.contains('.') {
            eslint_file = Some(t.to_string());
            continue;
        }
        if let (Some(f), true) = (&eslint_file, l.starts_with("  ")) {
            let (line, r) = digits(t);
            if let (Some(line), Some(r)) = (line, r.strip_prefix(':')) {
                let (col, r) = digits(r);
                let cols: Vec<&str> = r
                    .split("  ")
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .collect();
                if let [sev, msg, rule @ ..] = cols.as_slice() {
                    let msg = match rule.last() {
                        Some(rule) => format!("{msg} ({rule})"),
                        None => msg.to_string(),
                    };
                    push(&mut found, f, line, col, msg, *sev == "warning");
                }
                continue;
            }
        }
        if let Some((p, line, col, rest)) = tsc_paren(t).or_else(|| locate(t)) {
            if p.contains("node_modules/") {
                continue;
            }
            let (warn, msg) = severity(rest);
            let msg = match (msg.trim(), &jest_title) {
                ("" | ")", Some(title)) => title.clone(),
                ("" | ")", None) => continue,
                (m, _) => m.into(),
            };
            push(&mut found, p, line, col, msg, warn);
        }
    }
    found
}

fn push(
    found: &mut Vec<Problem>,
    file: &str,
    line: u32,
    col: Option<u32>,
    msg: String,
    warn: bool,
) {
    let file = file.trim_start_matches("./").to_string();
    let same_col = |c: Option<u32>| c.is_none() || col.is_none() || c == col;
    if found
        .iter()
        .any(|p| p.file == file && p.line == line && same_col(p.col))
    {
        return;
    }
    found.push(Problem {
        file,
        line,
        col,
        msg,
        warn,
    });
}

/// O caminho como a tarefa imprimiu, relativo à raiz da worktree (se for dela).
pub fn relative(root: &Path, file: &str) -> String {
    Path::new(file)
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file.to_string())
}

const MARK: &str = "@@maestro-fim-";

/// O comando como vai para o split: no fim imprime a marca com o código de saída.
/// O texto digitado tem `%s`, então a marca de verdade só aparece quando a tarefa termina.
pub fn wrapped(cmd: &str) -> String {
    format!("{cmd}; printf '\\n@@maestro-%s-%d@@\\n' fim $?\r")
}

/// (código de saída, saída da tarefa) quando a marca do fim já apareceu na tela do split.
pub fn finished(screen: &str) -> Option<(i32, &str)> {
    let end = screen.rfind(MARK)?;
    let code = screen[end + MARK.len()..]
        .split("@@")
        .next()?
        .trim()
        .parse()
        .ok()?;
    let start = screen[..end]
        .rfind("@@maestro-%s")
        .and_then(|i| screen[i..end].find('\n').map(|n| i + n + 1))
        .unwrap_or(0);
    Some((code, &screen[start.min(end)..end]))
}

/// Abre o split embaixo do `target` (o Claude da feature), roda e espera a marca do fim. Bloqueia.
pub fn run_in_split(
    herdr: &crate::herdr::Herdr,
    root: &Path,
    target: Option<String>,
    cmd: &str,
) -> Result<(i32, Vec<Problem>), String> {
    use serde_json::json;
    let p = herdr
        .call(
            "pane.split",
            json!({ "direction": "down", "cwd": root, "target_pane_id": target, "focus": false }),
        )
        .map_err(|e| e.to_string())?;
    let pane = p["pane"]["pane_id"]
        .as_str()
        .ok_or("o herdr não abriu o split")?
        .to_string();
    herdr
        .call(
            "pane.send_text",
            json!({ "pane_id": pane, "text": wrapped(cmd) }),
        )
        .map_err(|e| e.to_string())?;
    // ponytail: lê a tela a cada 1 s por até 30 min; o `pane.wait_for_output` estoura o timeout do socket
    for _ in 0..1800 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let r = herdr
            .call(
                "pane.read",
                json!({ "pane_id": pane, "source": "recent_unwrapped", "lines": 5000 }),
            )
            .map_err(|_| "o split da tarefa foi fechado antes de terminar".to_string())?;
        let text = r["read"]["text"]
            .as_str()
            .or(r["text"].as_str())
            .unwrap_or_default();
        if let Some((code, out)) = finished(text) {
            return Ok((code, problems(out)));
        }
    }
    Err("a tarefa passou de 30 min; os erros não foram lidos".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(out: &str) -> Vec<(String, u32, Option<u32>, bool, String)> {
        problems(out)
            .into_iter()
            .map(|p| (p.file, p.line, p.col, p.warn, p.msg))
            .collect()
    }

    #[test]
    fn saidas_reais() {
        let tsc = got(include_str!("fixtures/tasks/tsc.txt"));
        assert_eq!(
            tsc[0],
            (
                "src/a.ts".into(),
                4,
                Some(7),
                false,
                "TS2322: Type 'number' is not assignable to type 'string'.".into()
            )
        );
        assert_eq!(tsc.len(), 2);
        let pretty = got(include_str!("fixtures/tasks/tsc-pretty.txt"));
        assert_eq!(pretty.len(), 2);
        assert_eq!(
            pretty[1].4,
            "TS2339: Property 'foo' does not exist on type 'string'."
        );
        let es = got(include_str!("fixtures/tasks/eslint.txt"));
        assert_eq!(es.len(), 4);
        assert_eq!(
            es[3],
            (
                "/work/es/src/b.js".into(),
                3,
                Some(10),
                true,
                "'y' is not defined (no-undef)".into()
            )
        );
        let jest = got(include_str!("fixtures/tasks/jest.txt"));
        assert_eq!(
            jest,
            [("soma.test.js".into(), 2, Some(17), false, "soma".into())]
        );
        let cargo = got(include_str!("fixtures/tasks/cargo.txt"));
        assert_eq!(
            cargo,
            [(
                "src/main.rs".into(),
                2,
                Some(18),
                false,
                "E0308: mismatched types".into()
            )]
        );
        let gt = got(include_str!("fixtures/tasks/gotest.txt"));
        assert_eq!(
            gt,
            [(
                "soma_test.go".into(),
                7,
                None,
                false,
                "Soma(1, 1) = 2, want 3".into()
            )]
        );
        let gb = got(include_str!("fixtures/tasks/gobuild.txt"));
        assert_eq!(
            gb,
            [(
                "quebra.go".into(),
                4,
                Some(9),
                false,
                "undefined: naoExiste".into()
            )]
        );
        assert_eq!(
            relative(Path::new("/work/es"), "/work/es/src/b.js"),
            "src/b.js"
        );
    }

    #[test]
    fn fim_da_tarefa() {
        let typed = wrapped("npm test");
        assert!(typed.ends_with("fim $?\r") && !typed.contains(MARK));
        let screen = format!(
            "~/repo ❯ {}\n> x@1 test\nsrc/a.ts(1,1): error TS1: x\n\n@@maestro-fim-2@@\n~/repo ❯",
            typed.trim_end()
        );
        let (code, out) = finished(&screen).unwrap();
        assert_eq!(code, 2);
        assert_eq!(out, "> x@1 test\nsrc/a.ts(1,1): error TS1: x\n\n");
        assert!(finished(&format!("❯ {typed}")).is_none());
    }

    #[test]
    fn tarefas_do_repo() {
        let dir = std::env::temp_dir().join(format!("maestro-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"test":"jest","lint":"eslint ."}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Makefile"),
            "VAR := 1\n.PHONY: up\nup:\n\tdocker compose up\nx=1\n",
        )
        .unwrap();
        let names: Vec<_> = detect(&dir).into_iter().map(|t| t.cmd).collect();
        assert_eq!(names, ["npm test", "npm run lint", "make up"]);
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect(&dir)[1].cmd, "pnpm lint");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
