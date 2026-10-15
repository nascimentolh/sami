use std::{path::Path, process::Command};

use serde_json::Value;

use crate::{
    diff::{self, FileDiff, Kind},
    feature,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Run,
    Pass,
    Fail,
    /// não deu para checar (sem remoto, sem script…): não barra, mas também não é ✓
    Skip,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub state: State,
    pub detail: String,
}

/// Os itens do pré-voo (DESIGN §7.1), na ordem em que aparecem.
pub const ITEMS: [&str; 6] = [
    "Branch atualizada com a base",
    "Testes afetados",
    "Lint e typecheck",
    "Sonar local",
    "Nenhum segredo no diff",
    "Nenhum arquivo proibido",
];

fn check(state: State, detail: impl Into<String>) -> Check {
    Check {
        state,
        detail: detail.into(),
    }
}

/// Com teto de tempo: o que travar vira "não deu para checar" em vez de prender o pré-voo.
fn sh(root: &Path, cmd: &str) -> Check {
    let secs = 600;
    let out = Command::new("timeout")
        .args([&secs.to_string(), "sh", "-c", cmd])
        .current_dir(root)
        .output();
    match out {
        Ok(o) if o.status.code() == Some(124) => {
            check(State::Skip, format!("{cmd}: passou de {secs} s"))
        }
        Ok(o) if o.status.success() => check(State::Pass, cmd),
        Ok(o) => {
            let out = String::from_utf8_lossy(if o.stderr.is_empty() {
                &o.stdout
            } else {
                &o.stderr
            })
            .into_owned();
            let last = out
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            check(State::Fail, format!("{cmd}: {}", last.trim()))
        }
        Err(e) => check(State::Fail, format!("{cmd}: {e}")),
    }
}

fn behind(root: &Path, base: &str) -> Check {
    let has_remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_remote {
        return check(State::Skip, "sem remoto origin");
    }
    let fetched = Command::new("timeout")
        .args(["30", "git", "fetch", "-q", "origin", base])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success());
    if !fetched {
        return check(State::Skip, format!("git fetch origin {base} falhou"));
    }
    let n = Command::new("git")
        .args(["rev-list", "--count", &format!("HEAD..origin/{base}")])
        .current_dir(root)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        });
    match n {
        Some(0) => check(State::Pass, format!("em dia com origin/{base}")),
        Some(n) => check(
            State::Fail,
            format!(
                "{n} commit{} atrás de origin/{base}",
                if n > 1 { "s" } else { "" }
            ),
        ),
        None => check(State::Skip, "não deu para contar os commits"),
    }
}

fn lint(root: &Path) -> Check {
    if root.join("Cargo.toml").exists() {
        return sh(root, "cargo clippy -q --all-targets -- -D warnings");
    }
    let pkg: Value = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let scripts: Vec<&str> = ["lint", "typecheck", "type-check", "tsc"]
        .into_iter()
        .filter(|s| pkg["scripts"][s].is_string())
        .collect();
    if scripts.is_empty() {
        return check(
            State::Skip,
            "o repo não tem script de lint nem de typecheck",
        );
    }
    let cmd = scripts
        .iter()
        .map(|s| format!("npm run -s {s}"))
        .collect::<Vec<_>>()
        .join(" && ");
    sh(root, &cmd)
}

/// O MCP do Sonar só existe dentro do Claude Code: um `claude -p` curto nos arquivos mudados.
// ponytail: haiku, até 10 arquivos (~US$ 0,02); a análise completa continua no job `sonar` do pipeline
fn sonar(root: &Path, files: &[String]) -> Check {
    let pkg = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
    let ctbz = Command::new("sh")
        .args(["-c", "command -v ctbz-check"])
        .output()
        .is_ok_and(|o| o.status.success());
    if pkg.contains("\"vitest\"") && ctbz {
        let c = sh(root, "ctbz-check sonar-vitest");
        if c.state == State::Fail {
            return c;
        }
    }
    let code: Vec<&String> = files
        .iter()
        .filter(|f| {
            [".ts", ".tsx", ".js", ".jsx", ".py", ".java", ".go", ".kt"]
                .iter()
                .any(|e| f.ends_with(e))
        })
        .take(10)
        .collect();
    if code.is_empty() {
        return check(State::Skip, "nenhum arquivo de código que o Sonar analise");
    }
    let list = code
        .iter()
        .map(|f| f.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let prompt = format!(
        "Para cada arquivo ({list}): leia com Read e chame mcp__sonarqube__analyze_code_snippet com o conteúdo, a linguagem e o caminho relativo em filePath. \
         Responda SOMENTE um JSON {{\"issues\":[{{\"file\":\"\",\"line\":0,\"severity\":\"\",\"type\":\"\",\"message\":\"\"}}]}} \
         com os problemas encontrados (severity e type como o Sonar devolve); se a ferramenta der erro, {{\"error\":\"<mensagem>\"}}."
    );
    // na worktree (o container do MCP monta o diretório atual), mas sem os hooks dela
    // (`--setting-sources user`) e sem JSONL (senão vira um turno e entra no custo do card)
    let out = Command::new("timeout")
        .args(["180", "claude", "-p", &prompt])
        .args(["--model", "haiku", "--output-format", "json"])
        .arg("--allowedTools=Read,mcp__sonarqube__analyze_code_snippet")
        .args(["--setting-sources", "user", "--no-session-persistence"])
        .current_dir(root)
        .output();
    let Ok(out) = out else {
        return check(State::Skip, "claude não encontrado");
    };
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    sonar_reply(reply["result"].as_str().unwrap_or_default())
}

fn sonar_reply(result: &str) -> Check {
    let json = match (result.find('{'), result.rfind('}')) {
        (Some(a), Some(b)) if b > a => &result[a..=b],
        _ => return check(State::Skip, "o Claude não devolveu a análise do Sonar"),
    };
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    if let Some(e) = v["error"].as_str() {
        return check(State::Skip, format!("MCP do Sonar: {e}"));
    }
    let issues = v["issues"].as_array().cloned().unwrap_or_default();
    // barra o que o gate do Sonar barra: bug, vulnerabilidade, bloqueante ou crítico; o resto é aviso
    let serious = |i: &&Value| {
        let up = |k: &str| i[k].as_str().unwrap_or_default().to_uppercase();
        ["BLOCKER", "CRITICAL", "HIGH"].contains(&up("severity").as_str())
            || ["BUG", "VULNERABILITY"].contains(&up("type").as_str())
    };
    let bad: Vec<&Value> = issues.iter().filter(serious).collect();
    let n = |k: usize| if k > 1 { "s" } else { "" };
    match bad.first() {
        Some(i) => check(
            State::Fail,
            format!(
                "{} problema{} sério{} · {}:{} {}",
                bad.len(),
                n(bad.len()),
                n(bad.len()),
                i["file"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
                i["line"],
                i["message"].as_str().unwrap_or_default()
            ),
        ),
        None if issues.is_empty() => check(State::Pass, "nenhum problema nos arquivos mudados"),
        None => check(
            State::Pass,
            format!(
                "{} aviso{} menor{} (não barra)",
                issues.len(),
                n(issues.len()),
                if issues.len() > 1 { "es" } else { "" }
            ),
        ),
    }
}

fn token_after(line: &str, prefix: &str, min: usize, ok: fn(char) -> bool) -> bool {
    line.match_indices(prefix).any(|(i, _)| {
        !line[..i].ends_with(|c: char| c.is_ascii_alphanumeric())
            && line[i + prefix.len()..]
                .chars()
                .take_while(|c| ok(*c))
                .count()
                >= min
    })
}

/// (prefixo, tamanho mínimo do resto, caractere aceito, o que é)
type Prefix = (&'static str, usize, fn(char) -> bool, &'static str);

fn is_secret(line: &str) -> Option<&'static str> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let upper = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit();
    let prefixes: [Prefix; 8] = [
        ("glpat-", 20, word, "token do GitLab"),
        ("ghp_", 30, word, "token do GitHub"),
        ("github_pat_", 30, word, "token do GitHub"),
        ("xoxb-", 10, word, "token do Slack"),
        ("xoxp-", 10, word, "token do Slack"),
        ("sk-", 32, word, "chave de API"),
        ("AKIA", 16, upper, "chave da AWS"),
        ("AIza", 35, word, "chave do Google"),
    ];
    if let Some(p) = prefixes
        .iter()
        .find(|(p, min, ok, _)| token_after(line, p, *min, *ok))
    {
        return Some(p.3);
    }
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
        return Some("chave privada");
    }
    // `password = "…"`, `token: '…'`: valor literal longo, não variável de ambiente
    let lower = line.to_lowercase();
    let named = ["password", "passwd", "secret", "token", "api_key", "apikey"]
        .iter()
        .any(|k| lower.contains(k));
    if named {
        for q in ['"', '\''] {
            let mut parts = line.split(q);
            let before = parts.next().unwrap_or_default().trim_end();
            if let Some(value) = parts.next()
                && (before.ends_with(':') || before.ends_with('='))
                && value.len() >= 16
                && !value.contains(' ')
                && !value.contains("${")
            {
                return Some("segredo literal");
            }
        }
    }
    None
}

/// Linhas adicionadas (inclusive arquivos novos) que parecem segredo: o que o `/mr` commitaria.
pub fn secrets(files: &[FileDiff]) -> Vec<String> {
    files
        .iter()
        .flat_map(|f| {
            f.hunks.iter().flat_map(|h| &h.lines).filter_map(move |l| {
                (l.kind == Kind::Add)
                    .then(|| is_secret(&l.text))
                    .flatten()
                    .map(|k| format!("{}:{} {k}", f.path, l.new.unwrap_or_default()))
            })
        })
        .collect()
}

pub fn forbidden(files: &[FileDiff]) -> Vec<String> {
    files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| {
            let name = p.rsplit('/').next().unwrap_or(p);
            let env = name == ".env"
                || (name.starts_with(".env.")
                    && ![".env.example", ".env.sample", ".env.template"].contains(&name));
            env || p.split('/').any(|c| {
                [
                    "node_modules",
                    "dist",
                    "build",
                    "target",
                    "coverage",
                    ".next",
                ]
                .contains(&c)
            })
        })
        .map(String::from)
        .collect()
}

fn listed(found: Vec<String>, ok: &str) -> Check {
    match found.as_slice() {
        [] => check(State::Pass, ok),
        [one] => check(State::Fail, one.clone()),
        [first, rest @ ..] => check(State::Fail, format!("{first} e mais {}", rest.len())),
    }
}

/// Um item do pré-voo; cada um roda numa thread para a lista ir ficando verde aos poucos.
pub fn run(i: usize, root: &Path, base: &str) -> Check {
    let files = diff::worktree(root).map(|(_, f)| f).unwrap_or_default();
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    match i {
        0 => behind(root, base),
        1 => match diff::affected_tests(root, &paths).filter(|_| !paths.is_empty()) {
            Some(cmd) => sh(root, &cmd),
            None => check(State::Skip, "nenhum comando de teste para este repo"),
        },
        2 => lint(root),
        3 => sonar(root, &paths),
        4 => listed(secrets(&files), "nenhum token ou chave nas linhas novas"),
        _ => listed(forbidden(&files), "sem .env, build ou node_modules"),
    }
}

fn tree(root: &Path, commit: &str) -> Option<String> {
    let o = Command::new("git")
        .args(["rev-parse", &format!("{commit}^{{tree}}")])
        .current_dir(root)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Pré-voo aprovado (verde ou com override) para os arquivos como estão agora.
pub fn stamp(id: &str, root: &Path) -> Result<(), String> {
    let sha = feature::snapshot(root)?;
    let t = tree(root, &sha).ok_or("git rev-parse falhou")?;
    std::fs::write(feature::preflight_file(id), t).map_err(|e| e.to_string())
}

pub fn is_mr(prompt: &str) -> bool {
    let p = prompt.trim_start();
    p.strip_prefix("/mr")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// O hook do `/mr` digitado no terminal: sem pré-voo para estes arquivos, bloqueia.
/// Qualquer erro deixa passar: o hook nunca trava o Claude.
pub fn gate(id: &str, root: &Path, snapshot: &str, prompt: &str) -> Option<String> {
    if !is_mr(prompt) {
        return None;
    }
    let now = tree(root, snapshot)?;
    let stamped = std::fs::read_to_string(feature::preflight_file(id)).unwrap_or_default();
    (stamped.trim() != now).then(|| {
        "Pré-voo pendente para estes arquivos. Abra a aba MR da feature no Maestro (Ctrl+Shift+E → MR) e rode o pré-voo; ele manda o /mr quando passar.".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::{forbidden, is_mr, is_secret, secrets, sonar_reply};
    use crate::diff::parse;

    #[test]
    fn segredo_no_diff_barra() {
        // montado em tempo de execução para o push não acusar um token de verdade
        let fake = format!("glpat-{}", "x".repeat(20));
        let patch = format!(
            "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,1 +1,3 @@\n const a = 1\n+const t = \"{fake}\"\n+const u = process.env.TOKEN\n"
        );
        let files = parse(&patch);
        assert_eq!(secrets(&files), vec!["src/a.ts:2 token do GitLab"]);
        assert_eq!(
            is_secret(&format!("password: '{}'", "a".repeat(16))),
            Some("segredo literal")
        );
        assert_eq!(is_secret("const token = getToken()"), None);
        assert_eq!(
            is_secret(&format!("AKIA{}", "AB12".repeat(4))),
            Some("chave da AWS")
        );
    }

    #[test]
    fn arquivos_proibidos_e_mr() {
        let patch = [
            ".env",
            ".env.example",
            ".env.local",
            "dist/a.js",
            "src/build.ts",
        ]
        .iter()
        .map(|p| format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n@@ -0,0 +1 @@\n+x\n"))
        .collect::<String>();
        assert_eq!(
            forbidden(&parse(&patch)),
            vec![".env", ".env.local", "dist/a.js"]
        );
        assert!(is_mr("/mr") && is_mr("  /mr draft") && !is_mr("/mrx") && !is_mr("faz o /mr"));
        assert_eq!(sonar_reply("{\"issues\":[]}").state, super::State::Pass);
        let minor = "{\"issues\":[{\"file\":\"a.ts\",\"line\":3,\"severity\":\"MINOR\",\"type\":\"CODE_SMELL\",\"message\":\"x\"}]}";
        assert_eq!(sonar_reply(minor).state, super::State::Pass);
        let bug = minor.replace("CODE_SMELL", "BUG");
        assert_eq!(sonar_reply(&bug).state, super::State::Fail);
    }
}
