//! A auditoria (revisão 4 do plano): relatório salvo por sha, nunca automática, achados verificados e sem repetir work items.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{mrs, vault};

pub const PILLARS: &[(&str, &str)] = &[
    ("arq", "Arquitetura & Design"),
    ("sec", "Segurança"),
    ("qual", "Qualidade"),
    ("custo", "Custos & Performance"),
    ("hig", "Higiene"),
];

pub const SEVT: &[(&str, &str)] = &[
    ("P0", "perda de dados ou serviço fora em produção"),
    ("P1", "risco alto e provável"),
    ("P2", "risco real, raro ou contornável"),
    ("P3", "higiene"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "st", content = "ref", rename_all = "lowercase")]
pub enum St {
    #[default]
    New,
    /// mesmo padrão já corrigido num repo irmão (`email-service #19`)
    Cross(String),
    /// já existe neste repo (`#12`)
    Dup(String),
    Created(u64),
    /// fechado como won't do pelo Maestro
    Wont(u64),
    /// o MR da correção mergeou e o Maestro fechou
    Done(u64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub pillar: String,
    pub sev: String,
    pub labels: Vec<String>,
    pub title: String,
    pub location: String,
    pub confidence: String,
    pub problem: String,
    pub evidence: String,
    pub fix: String,
    pub impact: String,
    pub size: String,
    #[serde(default)]
    pub related: Option<String>,
    #[serde(default)]
    pub st: St,
    #[serde(default)]
    pub custom: Option<String>,
    #[serde(default)]
    pub on: bool,
}

impl Finding {
    /// `[P0] fix(worker): …`, o padrão dos work items do email-service.
    pub fn item_title(&self) -> String {
        format!("[{}] {}", self.sev, self.title)
    }

    /// O `body()` do protótipo.
    pub fn body(&self) -> String {
        if let Some(c) = &self.custom {
            return c.clone();
        }
        let mut b = format!(
            "## Problema\n\n{}\n\n## Evidência\n\n{}\n\n## Correção sugerida\n\n{}\n\n## Impacto\n\n{}",
            self.problem, self.evidence, self.fix, self.impact
        );
        if let St::Cross(r) = &self.st {
            b.push_str(&format!("\n\nRelacionado: {r}"));
        }
        b
    }

    pub fn open(&self) -> bool {
        !matches!(
            self.st,
            St::Dup(_) | St::Created(_) | St::Wont(_) | St::Done(_)
        )
    }

    pub fn iid(&self) -> Option<u64> {
        match self.st {
            St::Created(i) | St::Wont(i) | St::Done(i) => Some(i),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub sha: String,
    /// unix, para achar a última
    pub at: u64,
    pub date: String,
    /// `inc` ou `full`
    pub kind: String,
    pub cost: f64,
    pub findings: Vec<Finding>,
    pub dropped: Vec<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn dir(repo: &Path) -> PathBuf {
    repo.join(".maestro")
}

/// O último relatório salvo. É só isto (e `since`) que roda ao abrir o projeto.
pub fn latest(repo: &Path) -> Option<Report> {
    std::fs::read_dir(dir(repo).join("audits"))
        .ok()?
        .flatten()
        .filter_map(|e| serde_json::from_slice::<Report>(&std::fs::read(e.path()).ok()?).ok())
        .max_by_key(|r| r.at)
}

/// `.maestro/` fica fora do `git status` pelo `info/exclude`, sem mexer no `.gitignore` do repo.
fn exclude(repo: &Path) {
    let Some(common) = git(repo, &["rev-parse", "--git-common-dir"]) else {
        return;
    };
    let common = repo.join(common.trim());
    let f = common.join("info/exclude");
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    if !text.lines().any(|l| l.trim() == ".maestro/") {
        let _ = std::fs::create_dir_all(common.join("info"));
        let _ = std::fs::write(
            &f,
            format!(
                "{text}{}.maestro/\n",
                if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                }
            ),
        );
    }
}

pub fn save(repo: &Path, r: &Report) -> Result<(), String> {
    let d = dir(repo).join("audits");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    exclude(repo);
    std::fs::write(
        d.join(format!("{}.json", r.sha)),
        serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ignored {
    pub title: String,
    pub location: String,
}

fn ignore_file(repo: &Path) -> PathBuf {
    dir(repo).join("audit-ignore.json")
}

pub fn ignored(repo: &Path) -> Vec<Ignored> {
    std::fs::read(ignore_file(repo))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn set_ignored(repo: &Path, list: &[Ignored]) {
    let _ = std::fs::create_dir_all(dir(repo));
    exclude(repo);
    if let Ok(b) = serde_json::to_vec_pretty(list) {
        let _ = std::fs::write(ignore_file(repo), b);
    }
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

pub fn head(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string())
}

/// (commits, arquivos) desde `sha`. `None` se o sha não existe no repo.
pub fn since(repo: &Path, sha: &str) -> Option<(usize, usize)> {
    let range = format!("{sha}..HEAD");
    let commits = git(repo, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .ok()?;
    let files = git(repo, &["diff", "--name-only", &range])?.lines().count();
    Some((commits, files))
}

pub fn tracked(repo: &Path) -> usize {
    git(repo, &["ls-files"]).map_or(0, |s| s.lines().count())
}

/// ponytail: US$ 0,02 por arquivo vem de uma corrida real (79 arquivos, US$ 1,66); calibrar com os relatórios salvos
pub fn estimate(inc: bool, files: usize, total: usize) -> String {
    if inc {
        let usd = 0.1 + 0.02 * files as f64;
        format!(
            "{files} arquivos + vizinhos de 1 salto · ~US$ {} · ~{} min",
            brl(usd),
            (files / 6).max(3)
        )
    } else {
        let usd = 0.03 * total as f64;
        format!(
            "{total} arquivos, {} pilares + verificação · ~US$ {} a {} · ~{} min",
            PILLARS.len(),
            brl(usd * 0.7),
            brl(usd * 1.2),
            (total / 6).max(10)
        )
    }
}

pub fn brl(v: f64) -> String {
    format!("{v:.2}").replace('.', ",")
}

/// `git@gitlab.com:a/b.git` ou `https://gitlab.com/a/b.git` → `a/b`.
pub fn project_path(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches(".git");
    let p = if let Some(i) = r.find("://") {
        r[i + 3..].split_once('/')?.1
    } else {
        r.split_once(':')?.1
    };
    (!p.is_empty()).then(|| p.to_string())
}

pub fn project_of(repo: &Path) -> Option<String> {
    project_path(&git(repo, &["remote", "get-url", "origin"])?)
}

/// Um work item existente: deste repo (`repo` vazio) ou de um irmão.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub repo: String,
    pub iid: u64,
    pub title: String,
    pub closed: bool,
}

pub fn items_of(project: &str, repo: &str) -> Result<Vec<Item>, String> {
    let v = mrs::read(
        &format!(
            "projects/{}/issues?state=all&per_page=100",
            mrs::enc(project)
        ),
        true,
    )?;
    Ok(parse_items(&v, repo))
}

pub fn parse_items(v: &Value, repo: &str) -> Vec<Item> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|i| Item {
            repo: repo.into(),
            iid: i["iid"].as_u64().unwrap_or_default(),
            title: i["title"].as_str().unwrap_or_default().into(),
            closed: i["state"] == "closed",
        })
        .collect()
}

/// Os repos do mesmo `squad:` no vault (os irmãos cujos work items entram no dedup).
/// ponytail: só quem tem nota com `squad:` no vault; um `audit.toml` se precisar escolher na mão
pub fn siblings(notes: &[vault::Note], repo: &str) -> Vec<String> {
    let squad = |n: &vault::Note| {
        n.text
            .strip_prefix("---")?
            .split("\n---")
            .next()?
            .lines()
            .find_map(|l| l.strip_prefix("squad:"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let repos: Vec<&vault::Note> = notes.iter().filter(|n| n.dir == "repos").collect();
    let Some(mine) = repos.iter().find(|n| n.name == repo).and_then(|n| squad(n)) else {
        return Vec::new();
    };
    repos
        .iter()
        .filter(|n| n.name != repo && squad(n).as_deref() == Some(&mine))
        .map(|n| n.name.clone())
        .collect()
}

/// As decisões do vault em uma linha cada (`[[nome]]: primeira frase`).
pub fn decisions(notes: &[vault::Note]) -> String {
    notes
        .iter()
        .filter(|n| n.dir == "decisions")
        .map(|n| {
            let body = match n.text.strip_prefix("---") {
                Some(r) => r.split_once("\n---").map_or(r, |x| x.1),
                None => &n.text,
            };
            let first = body
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .unwrap_or_default();
            let first: String = first.chars().take(240).collect();
            format!("- [[{}]]: {first}", n.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const STOP: &[&str] = &[
    "com", "sem", "que", "para", "por", "uma", "dos", "das", "nos", "nas", "não", "the", "and",
    "cada", "toda", "todo", "mais", "quando", "deixa", "faz",
];

/// Os radicais do título sem o `[P0] tipo(escopo):`.
/// ponytail: radical = 5 primeiras letras; um stemmer de verdade se o dedup errar demais
fn stems(title: &str) -> (String, Vec<String>) {
    let t = title
        .trim_start_matches(|c| c != ']')
        .trim_start_matches(']');
    let t = if title.starts_with('[') {
        t.trim()
    } else {
        title
    };
    let (scope, rest) = match (t.find('('), t.find("):")) {
        (Some(a), Some(b)) if a < b && !t[..a].contains(' ') => {
            (t[a + 1..b].to_lowercase(), &t[b + 2..])
        }
        _ => (String::new(), t),
    };
    let mut out: Vec<String> = rest
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(w))
        .map(|w| w.chars().take(5).collect())
        .collect();
    out.sort();
    out.dedup();
    (scope, out)
}

fn similar(a: &str, b: &str) -> f64 {
    let ((sa, ta), (sb, tb)) = (stems(a), stems(b));
    let n = ta.iter().filter(|w| tb.contains(w)).count() as f64;
    let overlap = n / ta.len().min(tb.len()).max(1) as f64;
    // ponytail: mesmo escopo e 1/4 dos radicais, ou metade sem escopo; pesos se aparecer falso dup
    if (!sa.is_empty() && sa == sb && overlap >= 0.25) || overlap >= 0.5 {
        overlap + if sa == sb { 1. } else { 0. }
    } else {
        0.
    }
}

/// A rede de segurança da regra 3 do prompt: o que o Claude repetiu vira `dup`, o padrão de irmão vira `cross`.
pub fn dedup(f: &mut [Finding], items: &[Item]) {
    for x in f.iter_mut() {
        let best = |pred: &dyn Fn(&Item) -> bool| {
            items
                .iter()
                .filter(|i| pred(i))
                .map(|i| (similar(&x.title, &i.title), i))
                .filter(|(s, _)| *s > 0.)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, i)| i.clone())
        };
        if let Some(i) = best(&|i: &Item| i.repo.is_empty()) {
            x.st = St::Dup(format!("#{}", i.iid));
        } else if let Some(r) = x.related.clone().filter(|r| !r.trim().is_empty()) {
            x.st = St::Cross(r);
        } else if let Some(i) = best(&|i: &Item| !i.repo.is_empty() && i.closed) {
            x.st = St::Cross(format!("{} #{}", i.repo, i.iid));
        } else {
            x.st = St::New;
        }
        x.on = x.open();
    }
}

/// Tira o que você marcou como "não é problema"; devolve os títulos tirados.
pub fn drop_ignored(f: &mut Vec<Finding>, ign: &[Ignored]) -> Vec<String> {
    let mut out = Vec::new();
    f.retain(|x| {
        let hit = ign
            .iter()
            .any(|i| i.location == x.location || similar(&i.title, &x.title) > 0.);
        if hit {
            out.push(format!("{}: você marcou como não é problema", x.title));
        }
        !hit
    });
    out
}

pub const SCHEMA: &str = r#"{"type":"object","properties":{"findings":{"type":"array","items":{"type":"object","properties":{"pillar":{"type":"string","enum":["arq","sec","qual","custo","hig"]},"sev":{"type":"string","enum":["P0","P1","P2","P3"]},"labels":{"type":"array","items":{"type":"string","enum":["bug","security","tech-debt","scalability"]}},"title":{"type":"string"},"location":{"type":"string"},"confidence":{"type":"string","enum":["alta","média","baixa"]},"problem":{"type":"string"},"evidence":{"type":"string"},"fix":{"type":"string"},"impact":{"type":"string"},"size":{"type":"string","enum":["small","medium","large"]},"related":{"type":["string","null"]}},"required":["pillar","sev","labels","title","location","confidence","problem","evidence","fix","impact","size","related"]}}},"required":["findings"]}"#;

pub const VERIFY_SCHEMA: &str = r#"{"type":"object","properties":{"verdicts":{"type":"array","items":{"type":"object","properties":{"i":{"type":"integer"},"holds":{"type":"boolean"},"why":{"type":"string"}},"required":["i","holds","why"]}}},"required":["verdicts"]}"#;

pub struct Opts {
    pub inc: bool,
    pub base: String,
    pub pillars: Vec<&'static str>,
    pub budget: f64,
    pub prod: bool,
}

/// O prompt do snippet 5996330, melhorado (revisão 4 do plano).
pub fn prompt(
    repo: &str,
    o: &Opts,
    map: &str,
    known: &[Item],
    ign: &[Ignored],
    decisions: &str,
) -> String {
    let scope = if o.inc {
        format!(
            "incremental: só os arquivos de `git diff {}...HEAD` e quem os chama ou é chamado por eles (1 salto)",
            o.base
        )
    } else {
        "completo: o repo inteiro".into()
    };
    let pil = o
        .pillars
        .iter()
        .filter_map(|k| PILLARS.iter().find(|p| p.0 == *k))
        .map(|p| format!("{} ({})", p.1, p.0))
        .collect::<Vec<_>>()
        .join(", ");
    let known = known
        .iter()
        .map(|i| {
            if i.repo.is_empty() {
                format!("- #{} {}", i.iid, i.title)
            } else {
                format!(
                    "- {} #{} {}{}",
                    i.repo,
                    i.iid,
                    i.title,
                    if i.closed { " (corrigido)" } else { "" }
                )
            }
        })
        .chain(
            ign.iter()
                .map(|i| format!("- ignorado: {} ({})", i.title, i.location)),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let prod = if o.prod {
        "\nPara validar achados de custo e infra, pode consultar produção só com leitura: `gcloud logging read`, `gcloud sql|run|pubsub|container … describe/list`, `gcloud monitoring policies list` e `kubectl describe|top|logs` (sem `kubectl get`: ele lê Secrets) (use `--project`/`--context` de prd; descubra-os em k8s/, .gitlab-ci.yml e na nota do vault abaixo). Achado de custo/infra sem esse dado vira confiança baixa."
    } else {
        ""
    };
    format!(
        "Você é arquiteto de software e engenheiro de segurança sênior auditando {repo} em modo SOMENTE LEITURA.
Escopo: {scope}.
Leia também Dockerfile, .gitlab-ci.yml, k8s/, .env.example, README e CLAUDE.md. Siga o CLAUDE.md do repo.
Pilares: {pil}.{prod}

Regras:
1. Só reporte o que você VERIFICOU no código: cite arquivo:linha e o trecho. Suspeita sem evidência não entra.
2. Severidade por impacto: P0 perda de dado ou serviço fora em prd; P1 risco alto e provável; P2 real mas raro/contornável; P3 higiene.
3. NÃO repita nada desta lista (work items existentes e ignorados), nem reformulado com outras palavras:
{known}
   Se o achado repete um padrão já corrigido em outro repo da lista, preencha `related` com `repo #iid`; senão, null.
4. Correção sugerida concreta (trecho de código ou config), sem refatorações especulativas.
5. Não altere arquivos.
6. Não sugira o que contradiz estas decisões já tomadas:
{decisions}

title: `tipo(escopo): resumo`, sem a severidade (tipo: fix, sec, perf, infra ou chore). location: `arquivo:linha`.
problem, evidence, fix e impact em markdown curto, em português; evidence cita o trecho.

Mapa do repo (ripwire, ranqueado):
{map}"
    )
}

/// A segunda passada: cada achado contra o código. É o papel que o Codex fazia.
pub fn verify_prompt(f: &[Finding], decisions: &str) -> String {
    let list = f
        .iter()
        .enumerate()
        .map(|(i, x)| {
            format!(
                "{i}. [{}] {} — {}\nProblema: {}\nEvidência: {}",
                x.sev, x.title, x.location, x.problem, x.evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Verifique adversarialmente estes achados de auditoria. Para cada um, leia o código citado e responda: isto se sustenta? \
Descarte (holds=false) o que não se confirma no código, o que é falso positivo (query parametrizada, valor fake de teste, rota paginada) \
e o que contradiz uma destas decisões já tomadas (cite [[nome]] no why):
{decisions}

why: uma frase curta em português. Não altere arquivos.

{list}"
    )
}

/// Só leitura de verdade: `--restricted` ignora os settings (hooks e `allow` do usuário) e prende os arquivos ao repo;
/// o Bash só existe para os `git` de leitura (e, com `prod`, `gcloud`/`kubectl` de leitura).
pub fn claude_args(schema: &str, budget: f64, prod: bool) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--json-schema",
        schema,
        "--max-budget-usd",
        &format!("{budget:.2}"),
        "--no-session-persistence",
        "--restricted",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob,Bash",
        "--allowedTools",
        "Bash(git diff:*)",
        "Bash(git log:*)",
        "Bash(git blame:*)",
        "Bash(git show:*)",
        "Bash(git ls-files:*)",
        "Read",
        "Grep",
        "Glob",
    ]
    .map(String::from)
    .to_vec();
    if prod {
        a.extend(PROD_READ.iter().map(|c| format!("Bash({c}:*)")));
        // `kubectl get pods,secrets -o yaml` passa por qualquer prefixo `kubectl get <recurso>`; `describe` não mostra valores
        a.extend(["--disallowedTools", "Bash(kubectl get:*)"].map(String::from));
    }
    a
}

/// Produção pela CLI que você já usa (não há MCP de Cloud Monitoring/Logging configurado no claude).
/// ponytail: sem séries temporais do Cloud Monitoring (o gcloud não lê); `kubectl top` e os logs cobrem CPU/memória e erros
pub const PROD_READ: &[&str] = &[
    "gcloud logging read",
    "gcloud sql instances describe",
    "gcloud sql instances list",
    "gcloud run services describe",
    "gcloud run services list",
    "gcloud pubsub topics describe",
    "gcloud pubsub topics list",
    "gcloud pubsub subscriptions describe",
    "gcloud pubsub subscriptions list",
    "gcloud container clusters describe",
    "gcloud monitoring policies list",
    "kubectl describe",
    "kubectl top",
    "kubectl logs",
];

/// A linha `result` do stream-json: (saída estruturada, custo) ou (erro, custo).
pub fn parse_result(line: &str) -> Option<Result<(Value, f64), (String, f64)>> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "result" {
        return None;
    }
    let cost = v["total_cost_usd"].as_f64().unwrap_or(0.);
    let sub = v["subtype"].as_str().unwrap_or_default();
    if sub.contains("budget") {
        return Some(Err(("teto de gasto atingido".into(), cost)));
    }
    if v["is_error"] == true || sub != "success" {
        return Some(Err((
            v["result"].as_str().unwrap_or("o Claude falhou").into(),
            cost,
        )));
    }
    Some(Ok((v["structured_output"].clone(), cost)))
}

pub fn parse_findings(v: &Value) -> Vec<Finding> {
    v["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| serde_json::from_value::<Finding>(f.clone()).ok())
        .map(|mut f| {
            if f.title.starts_with("[P")
                && let Some((_, rest)) = f.title.split_once("] ")
            {
                f.title = rest.to_string();
            }
            f
        })
        .collect()
}

/// Separa os confirmados dos descartados (`título: descartado, porquê`).
pub fn apply_verdicts(f: Vec<Finding>, v: &Value) -> (Vec<Finding>, Vec<String>) {
    let verdicts: Vec<(usize, bool, String)> = v["verdicts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            (
                x["i"].as_u64().unwrap_or(u64::MAX) as usize,
                x["holds"] != false,
                x["why"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let (mut keep, mut dropped) = (Vec::new(), Vec::new());
    for (i, x) in f.into_iter().enumerate() {
        match verdicts.iter().find(|v| v.0 == i) {
            Some((_, false, why)) => dropped.push(format!(
                "{}: descartado, {}",
                x.title,
                why.trim_end_matches('.')
            )),
            _ => keep.push(x),
        }
    }
    (keep, dropped)
}

pub enum Ev {
    /// (classe do protótipo: `ok`, `sub`, `run` ou vazio; texto)
    Step(&'static str, String),
    Tool(String),
    Spent(f64),
    Pid(u32),
    Done(Report),
    Failed(String),
}

/// Um `claude -p` com o prompt no stdin; devolve a saída estruturada e o custo.
fn claude(
    repo: &Path,
    schema: &str,
    budget: f64,
    prod: bool,
    prompt: &str,
    send: &impl Fn(Ev),
) -> Result<(Value, f64), (String, f64)> {
    use std::io::{BufRead, Write};
    let mut child = Command::new("claude")
        .args(claude_args(schema, budget, prod))
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| (format!("claude: {e}"), 0.))?;
    send(Ev::Pid(child.id()));
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(prompt.as_bytes());
    }
    let mut res = None;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(r) = parse_result(&line) {
                res = Some(r);
            } else if let Some(mrs::Step::Tool(t)) = mrs::parse_event(&line) {
                send(Ev::Tool(t));
            }
        }
    }
    let _ = child.wait();
    res.unwrap_or_else(|| Err(("processo claude encerrado".into(), 0.)))
}

/// O pipeline: mapa → análise → verificação → dedup → relatório salvo. Roda numa thread.
pub fn run(repo: &Path, o: Opts, send: impl Fn(Ev)) {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(sha) = head(repo) else {
        return send(Ev::Failed("não é um repositório git".into()));
    };
    let total = tracked(repo);
    let map = Command::new("ripwire")
        .arg(repo)
        .arg("--max-tokens=6000")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    send(Ev::Step(
        "ok",
        format!("mapa do repo (ripwire): {total} arquivos"),
    ));
    if o.inc {
        let Some((_, files)) = since(repo, &o.base) else {
            return send(Ev::Failed(format!(
                "o sha {} não existe neste repo",
                o.base
            )));
        };
        send(Ev::Step(
            "ok",
            format!(
                "escopo: {files} arquivos mudaram desde {} + vizinhos de 1 salto",
                o.base
            ),
        ));
    } else {
        send(Ev::Step(
            "ok",
            "escopo: repo inteiro, Dockerfile, k8s e .gitlab-ci.yml incluídos".into(),
        ));
    }
    let notes = vault::load(&vault::dir());
    let mut known = match project_of(repo) {
        Some(p) => items_of(&p, "").unwrap_or_default(),
        None => Vec::new(),
    };
    let own = known.len();
    let sib = siblings(&notes, &name);
    let root = mrs::repos_root();
    for s in &sib {
        if let Some(p) = project_of(&root.join(s)) {
            known.extend(items_of(&p, s).unwrap_or_default());
        }
    }
    let dec = decisions(&notes);
    let ign = ignored(repo);
    send(Ev::Step(
        "ok",
        format!(
            "contexto: {own} work items deste repo{}, {} decisões do vault, {} ignorados",
            if sib.is_empty() {
                String::new()
            } else {
                format!(" + {} de {}", known.len() - own, sib.join(" e "))
            },
            notes.iter().filter(|n| n.dir == "decisions").count(),
            ign.len()
        ),
    ));
    let pil: Vec<&str> = o
        .pillars
        .iter()
        .filter_map(|k| PILLARS.iter().find(|p| p.0 == *k).map(|p| p.1))
        .collect();
    send(Ev::Step("run", format!("análise: {}", pil.join(", "))));
    // ponytail: uma chamada para todos os pilares; uma por pilar se a qualidade cair em repo grande
    let mut p = prompt(&name, &o, &map, &known, &ign, &dec);
    if o.prod {
        if let Some(n) = vault::find(&notes, &name) {
            p.push_str(&format!("\n\nNota do vault sobre {name}:\n{}", n.text));
        }
        send(Ev::Step(
            "ok",
            "dados de produção: gcloud e kubectl liberados só para leitura".into(),
        ));
    }
    let (raw, c1) = match claude(repo, SCHEMA, o.budget, o.prod, &p, &send) {
        Ok(r) => r,
        Err((e, c)) => {
            send(Ev::Spent(c));
            return send(Ev::Failed(e));
        }
    };
    send(Ev::Spent(c1));
    let raw = parse_findings(&raw);
    for (k, label) in PILLARS.iter().filter(|p| o.pillars.contains(&p.0)) {
        let n = raw.iter().filter(|f| f.pillar == *k).count();
        send(Ev::Step(
            "sub",
            format!("{label}: {n} achado{} bruto{}", s(n), s(n)),
        ));
    }
    let left = o.budget - c1;
    let (mut kept, mut dropped, c2) = if raw.is_empty() {
        (raw, Vec::new(), 0.)
    } else if left < 0.05 {
        return send(Ev::Failed("teto de gasto atingido".into()));
    } else {
        send(Ev::Step(
            "run",
            format!("verificação: {} achados contra o código", raw.len()),
        ));
        match claude(
            repo,
            VERIFY_SCHEMA,
            left,
            o.prod,
            &verify_prompt(&raw, &dec),
            &send,
        ) {
            Ok((v, c)) => {
                let n = raw.len();
                let (k, d) = apply_verdicts(raw, &v);
                send(Ev::Step(
                    "ok",
                    format!(
                        "verificação: {n} achados brutos → {} confirmados; {} descartados",
                        k.len(),
                        d.len()
                    ),
                ));
                (k, d, c)
            }
            Err((e, c)) => {
                send(Ev::Spent(c1 + c));
                return send(Ev::Failed(e));
            }
        }
    };
    send(Ev::Spent(c1 + c2));
    dropped.extend(drop_ignored(&mut kept, &ign));
    dedup(&mut kept, &known);
    let dup = kept.iter().filter(|f| matches!(f.st, St::Dup(_))).count();
    let cross = kept.iter().filter(|f| matches!(f.st, St::Cross(_))).count();
    send(Ev::Step(
        "ok",
        format!(
            "dedup contra {} work items: {dup} já existe{}, {cross} repete{} padrão já corrigido",
            known.len(),
            s(dup),
            if cross == 1 { "" } else { "m" }
        ),
    ));
    let r = Report {
        sha,
        at: now(),
        date: vault::today(),
        kind: if o.inc { "inc" } else { "full" }.into(),
        cost: c1 + c2,
        findings: kept,
        dropped,
    };
    if let Err(e) = save(repo, &r) {
        return send(Ev::Failed(format!("salvar o relatório: {e}")));
    }
    send(Ev::Done(r));
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Abre o work item. Só passa pelo `mrs::write` (o único caminho de escrita no GitLab).
pub fn create(project: &str, f: &Finding) -> Result<u64, String> {
    let v = mrs::write(
        "POST",
        &format!("projects/{}/issues", mrs::enc(project)),
        &json!({
            "title": f.item_title(),
            "labels": f.labels.join(","),
            "description": f.body(),
        }),
    )?;
    v["iid"]
        .as_u64()
        .ok_or_else(|| "o GitLab não devolveu o iid".into())
}

/// O work item como está no GitLab, com as notas de gente (sem as de sistema).
#[derive(Debug, Clone, PartialEq)]
pub struct Wi {
    pub body: String,
    pub closed: bool,
    pub web: String,
    /// (autor, data `2026-10-02`, texto)
    pub notes: Vec<(String, String, String)>,
}

pub fn work_item(project: &str, iid: u64) -> Result<Wi, String> {
    let base = format!("projects/{}/issues/{iid}", mrs::enc(project));
    let i = mrs::read(&base, false)?;
    let n = mrs::read(&format!("{base}/notes?sort=asc&per_page=100"), true)?;
    Ok(parse_wi(&i, &n))
}

pub fn parse_wi(i: &Value, n: &Value) -> Wi {
    Wi {
        body: i["description"].as_str().unwrap_or_default().into(),
        closed: i["state"] == "closed",
        web: i["web_url"].as_str().unwrap_or_default().into(),
        notes: n
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["system"] != true)
            .map(|x| {
                (
                    x["author"]["username"].as_str().unwrap_or_default().into(),
                    x["created_at"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(10)
                        .collect(),
                    x["body"].as_str().unwrap_or_default().into(),
                )
            })
            .collect(),
    }
}

pub fn comment(project: &str, iid: u64, text: &str) -> Result<(), String> {
    mrs::write(
        "POST",
        &format!("projects/{}/issues/{iid}/notes", mrs::enc(project)),
        &json!({ "body": text }),
    )
    .map(|_| ())
}

fn put(project: &str, iid: u64, body: Value) -> Result<(), String> {
    mrs::write(
        "PUT",
        &format!("projects/{}/issues/{iid}", mrs::enc(project)),
        &body,
    )
    .map(|_| ())
}

/// Fecha com a label `wont-do`; o motivo vai antes, como comentário.
pub fn wont_do(project: &str, iid: u64, why: &str) -> Result<(), String> {
    if !why.trim().is_empty() {
        comment(project, iid, &format!("Won't do: {}", why.trim()))?;
    }
    put(
        project,
        iid,
        json!({ "state_event": "close", "add_labels": "wont-do" }),
    )
}

pub fn reopen(project: &str, iid: u64) -> Result<(), String> {
    put(
        project,
        iid,
        json!({ "state_event": "reopen", "remove_labels": "wont-do" }),
    )
}

const START_Q: &str = "query($p:ID!,$iid:String!){currentUser{id} project(fullPath:$p){workItems(iids:[$iid]){nodes{id workItemType{widgetDefinitions{... on WorkItemWidgetDefinitionStatus{allowedStatuses{id category}}}}}}}}";
const START_M: &str = "mutation($in:WorkItemUpdateInput!){workItemUpdate(input:$in){errors}}";

/// Status pela categoria (`in_progress`, `done`), não pelo nome: vale para status customizados.
/// Sem o widget de status (GitLab Free), só o assignee muda.
fn set_wi(project: &str, iid: u64, category: &str, assign: bool) -> Result<(), String> {
    let v = mrs::write(
        "POST",
        "graphql",
        &json!({ "query": START_Q, "variables": { "p": project, "iid": iid.to_string() } }),
    )?;
    let (me, wi, status) = start_ids(&v, category)?;
    let mut input = json!({ "id": wi });
    if assign {
        input["assigneesWidget"] = json!({ "assigneeIds": [me] });
    }
    if let Some(s) = status {
        input["statusWidget"] = json!({ "status": s });
    }
    mrs::write(
        "POST",
        "graphql",
        &json!({ "query": START_M, "variables": { "in": input } }),
    )
    .map(|_| ())
}

/// "Corrigir em feature": comenta, põe você de assignee e o status em andamento.
pub fn start(project: &str, iid: u64, branch: &str) -> Result<(), String> {
    comment(
        project,
        iid,
        &format!("Correção em andamento na branch `{branch}`, aberta pelo Maestro."),
    )?;
    set_wi(project, iid, "in_progress", true)
}

/// O MR da `fix/<iid>-…` de um achado aberto foi mergeado: comenta, status Done, fecha e
/// marca o achado como corrigido no relatório. `None` se a branch não veio de um achado.
pub fn finish_merged(repo: &Path, branch: &str, mr: u64) -> Result<Option<u64>, String> {
    let Some(mut r) = latest(repo) else {
        return Ok(None);
    };
    let Some(f) = r.findings.iter_mut().find(|f| match f.st {
        St::Created(n) => fix_branch(n, &f.title) == branch,
        _ => false,
    }) else {
        return Ok(None);
    };
    let St::Created(iid) = f.st else {
        return Ok(None);
    };
    let project = project_of(repo).ok_or("o origin não é do GitLab")?;
    comment(
        &project,
        iid,
        &format!("Corrigido no !{mr}, mergeado. Fechado pelo Maestro."),
    )?;
    set_wi(&project, iid, "done", false)?;
    put(&project, iid, json!({ "state_event": "close" }))?;
    f.st = St::Done(iid);
    save(repo, &r)?;
    Ok(Some(iid))
}

/// Os work items fechados no GitLab, por fora do Maestro também (`Closes #N` do MR, à mão):
/// (iid, fechado como won't do).
pub fn closed(project: &str, iids: &[u64]) -> Result<Vec<(u64, bool)>, String> {
    let q: String = iids.iter().map(|i| format!("&iids[]={i}")).collect();
    let v = mrs::read(
        &format!(
            "projects/{}/issues?state=closed&per_page=100{q}",
            mrs::enc(project)
        ),
        true,
    )?;
    Ok(closed_of(&v))
}

pub fn closed_of(v: &Value) -> Vec<(u64, bool)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let wont = i["labels"]
                .as_array()
                .is_some_and(|l| l.iter().any(|x| x == "wont-do"));
            Some((i["iid"].as_u64()?, wont))
        })
        .collect()
}

/// (eu, o work item, o status da categoria se o tipo tiver status)
pub fn start_ids(v: &Value, category: &str) -> Result<(String, String, Option<String>), String> {
    let d = &v["data"];
    let me = d["currentUser"]["id"]
        .as_str()
        .ok_or("sem usuário no glab")?;
    let n = &d["project"]["workItems"]["nodes"][0];
    let wi = n["id"].as_str().ok_or("work item não encontrado")?;
    let status = n["workItemType"]["widgetDefinitions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w["allowedStatuses"].as_array())
        .flatten()
        .find(|s| s["category"] == category)
        .and_then(|s| s["id"].as_str())
        .map(String::from);
    Ok((me.into(), wi.into(), status))
}

/// Troca a descrição e comenta o que mudou.
pub fn update_fix(project: &str, iid: u64, body: &str, note: &str) -> Result<(), String> {
    put(project, iid, json!({ "description": body }))?;
    comment(project, iid, note)
}

/// O corpo com a seção `## Correção sugerida` trocada (ou somada no fim, se não houver).
pub fn replace_fix(body: &str, fix: &str) -> String {
    const H: &str = "## Correção sugerida";
    let Some(i) = body.find(H) else {
        return format!("{}\n\n{H}\n\n{}", body.trim_end(), fix.trim());
    };
    let rest = &body[i + H.len()..];
    let end = rest.find("\n## ").map_or(body.len(), |j| i + H.len() + j);
    format!("{}{H}\n\n{}\n{}", &body[..i], fix.trim(), &body[end..])
}

pub const ITER_SCHEMA: &str = r#"{"type":"object","properties":{"fix":{"type":"string"},"why":{"type":"string"}},"required":["fix","why"]}"#;

pub fn iter_prompt(iid: u64, w: &Wi, ask: &str) -> String {
    let notes: String = w
        .notes
        .iter()
        .map(|(who, when, t)| format!("- {who} ({when}): {t}\n"))
        .collect();
    format!(
        "Este é o work item #{iid} aberto por uma auditoria deste repositório:

{}

Comentários:
{}
Pedido: {ask}

Leia o código citado e reescreva só a correção sugerida, atendendo ao pedido e aos comentários. \
fix: o novo texto da seção \"Correção sugerida\" em markdown curto (sem o título), com arquivo:linha quando ajudar. \
why: uma frase em português dizendo o que mudou e por quê. Não altere arquivos.",
        w.body,
        if notes.is_empty() { "(nenhum)\n".into() } else { notes }
    )
}

/// Iterar com o Claude: só leitura, como a auditoria; devolve (correção, por quê, custo).
pub fn iterate(repo: &Path, iid: u64, w: &Wi, ask: &str) -> Result<(String, String, f64), String> {
    let (v, cost) = claude(
        repo,
        ITER_SCHEMA,
        1.,
        false,
        &iter_prompt(iid, w, ask),
        &|_| {},
    )
    .map_err(|(e, _)| e)?;
    match (v["fix"].as_str(), v["why"].as_str()) {
        (Some(f), Some(y)) if !f.trim().is_empty() => Ok((f.into(), y.into(), cost)),
        _ => Err("o Claude não devolveu uma correção".into()),
    }
}

/// `fix/<iid>-slug` do `fixInFeature` do protótipo.
pub fn fix_branch(iid: u64, title: &str) -> String {
    let rest = title.split_once(": ").map_or(title, |x| x.1);
    let s: String = crate::feature::slug(rest)
        .to_lowercase()
        .chars()
        .take(32)
        .collect();
    format!("fix/{iid}-{}", s.trim_end_matches('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(title: &str) -> Finding {
        Finding {
            pillar: "arq".into(),
            sev: "P1".into(),
            labels: vec!["bug".into()],
            title: title.into(),
            location: "src/x.ts:1".into(),
            confidence: "alta".into(),
            problem: "p".into(),
            evidence: "e".into(),
            fix: "f".into(),
            impact: "i".into(),
            size: "small".into(),
            related: None,
            st: St::New,
            custom: None,
            on: false,
        }
    }

    fn email_items(repo: &str) -> Vec<Item> {
        parse_items(
            &serde_json::from_str(include_str!("fixtures/email-service-issues.json")).unwrap(),
            repo,
        )
    }

    #[test]
    fn reworded_email_service_items_come_back_as_dup() {
        let items = email_items("");
        let mut list = vec![
            f("fix(worker): handler faz ack() no catch e descarta a mensagem"),
            f("sec(logs): corpo do e-mail logado inteiro em INFO"),
            f("perf(queue): getJobs carrega a fila toda a cada enfileiramento"),
            f("infra(k8s): limite de memória sem --max-old-space-size"),
            f("fix(db): pool.connect() nunca libera a conexão"),
            f("sec(api): rotas POST /send e /reprocess sem autenticação"),
            f("chore(env): typo STMP no nome das variáveis SMTP"),
            f("fix(template): cache de templates nunca expira"),
            f("fix(worker): retry sem backoff exponencial"),
        ];
        dedup(&mut list, &items);
        let st: Vec<_> = list.iter().map(|x| x.st.clone()).collect();
        assert_eq!(
            st,
            vec![
                St::Dup("#19".into()),
                St::Dup("#27".into()),
                St::Dup("#28".into()),
                St::Dup("#30".into()),
                St::Dup("#15".into()),
                St::Dup("#25".into()),
                St::Dup("#46".into()),
                St::New,
                St::New,
            ]
        );
        assert!(!list[0].on && list[7].on);
    }

    #[test]
    fn sibling_pattern_is_cross_not_dup() {
        let mut list = vec![
            f("fix(worker): erro no handler faz ack e descarta a mensagem do Pub/Sub"),
            f("chore(env): typo SLAK_TOKEN no .env.example"),
        ];
        list[1].related = Some("contato-service #20".into());
        dedup(&mut list, &email_items("email-service"));
        assert_eq!(list[0].st, St::Cross("email-service #19".into()));
        assert_eq!(list[1].st, St::Cross("contato-service #20".into()));
        assert!(list[0].body().ends_with("Relacionado: email-service #19"));
        assert_eq!(
            list[0].item_title(),
            "[P1] fix(worker): erro no handler faz ack e descarta a mensagem do Pub/Sub"
        );
    }

    #[test]
    fn ignored_and_verdicts() {
        let mut list = vec![f("fix(a): um"), f("fix(b): dois"), f("fix(c): três")];
        list[2].location = "Dockerfile:18".into();
        let gone = drop_ignored(
            &mut list,
            &[Ignored {
                title: "outra coisa".into(),
                location: "Dockerfile:18".into(),
            }],
        );
        assert_eq!(gone.len(), 1);
        let (keep, dropped) = apply_verdicts(
            list,
            &json!({"verdicts":[{"i":0,"holds":true,"why":"ok"},{"i":1,"holds":false,"why":"usa query parametrizada."}]}),
        );
        assert_eq!(keep.len(), 1);
        assert_eq!(
            dropped,
            vec!["fix(b): dois: descartado, usa query parametrizada"]
        );
    }

    #[test]
    fn paths_and_results() {
        assert_eq!(
            project_path("git@gitlab.com:ctbz/experts/email-service.git").as_deref(),
            Some("ctbz/experts/email-service")
        );
        assert_eq!(
            project_path("https://gitlab.com/a/b.git\n").as_deref(),
            Some("a/b")
        );
        assert_eq!(
            fix_branch(
                31,
                "fix(worker): erro no handler faz ack e descarta a mensagem"
            ),
            "fix/31-erro-no-handler-faz-ack-e-descar"
        );
        assert!(matches!(
            parse_result(r#"{"type":"result","subtype":"error_max_budget_usd","total_cost_usd":2.01}"#),
            Some(Err((ref e, c))) if e == "teto de gasto atingido" && c > 2.
        ));
        let ok = parse_result(
            r#"{"type":"result","subtype":"success","total_cost_usd":0.5,"structured_output":{"findings":[{"pillar":"sec","sev":"P1","labels":["security"],"title":"[P1] sec(logs): x","location":"a:1","confidence":"alta","problem":"p","evidence":"e","fix":"f","impact":"i","size":"small","related":null}]}}"#,
        );
        let Some(Ok((v, _))) = ok else { panic!() };
        let a = claude_args("{}", 1., true);
        assert!(a.contains(&"Bash(gcloud logging read:*)".to_string()));
        assert!(
            !a.iter()
                .any(|x| x.starts_with("Bash(kubectl get") && x != "Bash(kubectl get:*)")
        );
        assert!(
            a.windows(2)
                .any(|w| w == ["--disallowedTools", "Bash(kubectl get:*)"])
        );
        assert!(!a.iter().any(|x| x.contains("gcloud")
            && !x.contains(" list")
            && !x.contains(" describe")
            && !x.contains(" read")));
        assert!(
            !claude_args("{}", 1., false)
                .iter()
                .any(|x| x.contains("gcloud"))
        );
        assert_eq!(parse_findings(&v)[0].title, "sec(logs): x");
    }

    #[test]
    fn fechados_por_fora() {
        assert_eq!(
            closed_of(&json!([{"iid":3,"labels":["security"]},{"iid":4,"labels":["wont-do"]}])),
            [(3, false), (4, true)]
        );
    }

    #[test]
    fn em_andamento() {
        let v = json!({"data":{"currentUser":{"id":"gid://gitlab/User/1"},"project":{"workItems":{"nodes":[{"id":"gid://gitlab/WorkItem/9","workItemType":{"widgetDefinitions":[{},{"allowedStatuses":[{"id":"s1","category":"to_do"},{"id":"s2","category":"in_progress"}]}]}}]}}}});
        assert_eq!(
            start_ids(&v, "in_progress").unwrap(),
            (
                "gid://gitlab/User/1".into(),
                "gid://gitlab/WorkItem/9".into(),
                Some("s2".into())
            )
        );
        let free = json!({"data":{"currentUser":{"id":"u"},"project":{"workItems":{"nodes":[{"id":"w","workItemType":{"widgetDefinitions":[{}]}}]}}}});
        assert_eq!(start_ids(&free, "done").unwrap().2, None);
        let none = json!({"data":{"currentUser":{"id":"u"},"project":{"workItems":{"nodes":[]}}}});
        assert!(start_ids(&none, "done").is_err());
        assert_eq!(
            mrs::gql_ok(r#"{"data":{"workItemUpdate":{"errors":["sem permissão"]}}}"#),
            Err("sem permissão".into())
        );
        assert!(mrs::gql_ok(r#"{"errors":[{"message":"x"}]}"#).is_err());
        assert!(mrs::gql_ok(r#"{"data":{"workItemUpdate":{"errors":[]}}}"#).is_ok());
    }

    #[test]
    fn work_item_e_correcao() {
        let w = parse_wi(
            &json!({"description":"## Problema\n\na\n\n## Correção sugerida\n\nvelha\n\n## Impacto\n\nb","state":"closed","web_url":"u"}),
            &json!([{"system":true,"body":"x"},{"system":false,"author":{"username":"ana"},"created_at":"2026-10-02T10:00:00Z","body":"e o redact?"}]),
        );
        assert!(w.closed);
        assert_eq!(
            w.notes,
            [("ana".into(), "2026-10-02".into(), "e o redact?".into())]
        );
        assert_eq!(
            replace_fix(&w.body, "nova"),
            "## Problema\n\na\n\n## Correção sugerida\n\nnova\n\n## Impacto\n\nb"
        );
        assert_eq!(
            replace_fix("## Problema\n\na\n\n## Correção sugerida\n\nvelha", "nova"),
            "## Problema\n\na\n\n## Correção sugerida\n\nnova\n"
        );
        assert_eq!(
            replace_fix("só texto", "nova"),
            "só texto\n\n## Correção sugerida\n\nnova"
        );
        assert!(iter_prompt(28, &w, "basta o redact?").contains("ana (2026-10-02): e o redact?"));
        assert!(
            !Finding {
                st: St::Wont(3),
                ..f("x")
            }
            .open()
        );
    }

    #[test]
    fn report_round_trip_and_exclude() {
        let d = std::env::temp_dir().join(format!("maestro-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&d)
            .status()
            .unwrap();
        let mut x = f("fix(a): um");
        x.st = St::Created(31);
        let r = Report {
            sha: "abc".into(),
            at: 2,
            date: "2026-10-15".into(),
            kind: "inc".into(),
            cost: 0.8,
            findings: vec![x],
            dropped: vec![],
        };
        save(
            &d,
            &Report {
                sha: "old".into(),
                at: 1,
                ..r.clone()
            },
        )
        .unwrap();
        save(&d, &r).unwrap();
        save(&d, &r).unwrap();
        assert_eq!(latest(&d), Some(r));
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches(".maestro/").count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
