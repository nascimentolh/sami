//! News do daily.dev filtrado pelo que você usa: perfil dos repos, coleta, pontuação e o que fica só local.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::vault::Note;

const API: &str = "https://api.daily.dev/public/v1";
/// no máximo uma coleta a cada 3 h: a API dá 60 chamadas por minuto por usuário
pub const EVERY: u64 = 3 * 3600;
/// abaixo disso o item não aparece
const MIN_SCORE: f32 = 0.3;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn env_or(var: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(var).map_or(default, PathBuf::from)
}

pub fn token_file() -> PathBuf {
    env_or(
        "MAESTRO_NEWS_TOKEN",
        home().join(".config/maestro/dailydev.token"),
    )
}

fn cache_file() -> PathBuf {
    env_or(
        "MAESTRO_NEWS_CACHE",
        home().join(".cache/maestro/news.json"),
    )
}

fn ignore_file() -> PathBuf {
    env_or(
        "MAESTRO_NEWS_IGNORE",
        home().join(".config/maestro/news-ignore.json"),
    )
}

pub fn has_token() -> bool {
    std::fs::read_to_string(token_file()).is_ok_and(|t| !t.trim().is_empty())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub title: String,
    pub url: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub source: String,
    /// segundos desde a época (`publishedAt`, ou `createdAt` quando falta)
    pub published: u64,
    pub read_time: u32,
}

impl Post {
    /// O título numa linha: posts que são tweets trazem o texto inteiro como título.
    pub fn headline(&self) -> String {
        let line = self
            .title
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default();
        match line.char_indices().nth(100) {
            Some((i, _)) => format!("{}…", line[..i].trim_end()),
            None => line.to_string(),
        }
    }
}

fn post(v: &Value) -> Option<Post> {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Some(Post {
        id: v["id"].as_str()?.to_string(),
        title: s("title"),
        url: s("url"),
        summary: s("summary"),
        tags: v["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(str::to_lowercase))
            .collect(),
        source: v["source"]["name"].as_str().unwrap_or_default().into(),
        published: v["publishedAt"]
            .as_str()
            .or(v["createdAt"].as_str())
            .and_then(epoch)
            .unwrap_or_default(),
        read_time: v["readTime"].as_u64().unwrap_or_default() as u32,
    })
}

/// `2026-10-04T13:22:49.223Z` → segundos desde a época (UTC).
pub fn epoch(iso: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (
        n(11..13).unwrap_or(0),
        n(14..16).unwrap_or(0),
        n(17..19).unwrap_or(0),
    );
    // dias desde 1970-01-01 (algoritmo civil de Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// "há 5 h", "ontem", "há 3 dias"
pub fn ago(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..3600 => format!("há {} min", (s / 60).max(1)),
        3600..86400 => format!("há {} h", s / 3600),
        86400..172800 => "ontem".into(),
        _ => format!("há {} dias", s / 86400),
    }
}

// ---------- perfil ----------

/// Uma dependência (ou base de imagem, ou infra) e os repos que a usam.
#[derive(Debug, Clone, PartialEq)]
pub struct Dep {
    /// `@nestjs/core 10`, `node:24`, `GitLab CI`
    pub label: String,
    /// como aparece num post: tag do daily.dev ou palavra no título/resumo
    pub keys: Vec<String>,
    pub repos: Vec<String>,
    /// GitLab CI, Cloud Run, k8s: plataforma, não código seu; um CVE nelas não "afeta" os repos
    pub infra: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub deps: Vec<Dep>,
    /// notas de `decisions/`: o "relacionado no vault"
    pub decisions: Vec<String>,
    /// notas de `repos/`
    pub repo_notes: Vec<String>,
}

/// Escopos que só agrupam: o que diz o assunto é o nome do pacote (`@google-cloud/pubsub` → pubsub).
const GENERIC_SCOPES: &[&str] = &["google-cloud", "aws-sdk", "azure", "types", "babel"];
const NOISE: &[&str] = &[
    "eslint",
    "prettier",
    "husky",
    "lint-staged",
    "dotenv",
    "rimraf",
    "ts-node",
    "tslib",
    "reflect-metadata",
    "source-map-support",
    "cross-env",
    "nodemon",
    "uuid",
    "lodash",
    "npm-run-all",
    "envsub",
    "tsconfig-paths",
    "type-provider",
    "-dom",
    "concurrently",
    "private",
    "contabilizei",
    "ctbz",
    "babel",
];
const GENERIC_NAMES: &[&str] = &[
    "core",
    "common",
    "utils",
    "config",
    "testing",
    "client",
    "cli",
    "platform-express",
    "schematics",
    "date",
    "google",
    "preset",
    "node",
    "type",
    "types",
    "storage",
    "logging",
    "tasks",
];

/// `@nestjs/core` → `nestjs`; `@google-cloud/pubsub` → `pubsub`; `ts-jest` → `ts-jest`.
fn npm_key(name: &str) -> Option<String> {
    if NOISE.iter().any(|n| name.contains(n)) || name.starts_with("@types/") || name == "tsx" {
        return None;
    }
    let key = match name.strip_prefix('@').and_then(|s| s.split_once('/')) {
        Some((scope, pkg)) if GENERIC_SCOPES.contains(&scope) => pkg,
        Some((scope, _)) => scope,
        None => name,
    };
    (key.len() >= 3 && !GENERIC_NAMES.contains(&key)).then(|| key.to_lowercase())
}

/// `^10.2.0` → `10`
fn major(v: &str) -> Option<String> {
    let d: String = v
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!d.is_empty()).then_some(d)
}

/// `drizzle-orm` e `drizzle-kit` são o mesmo assunto; `fastify-plugin` é do `fastify`.
fn family(key: &str, all: &BTreeSet<String>) -> String {
    let Some((pre, _)) = key.split_once('-') else {
        return key.into();
    };
    let siblings = all
        .iter()
        .filter(|k| k.starts_with(&format!("{pre}-")))
        .count();
    if pre.len() >= 4 && !GENERIC_NAMES.contains(&pre) && (all.contains(pre) || siblings >= 2) {
        pre.into()
    } else {
        key.into()
    }
}

/// O rótulo do perfil: o `/core` do escopo, senão o mais usado; escopo de muitos pacotes vira `@escopo/*`.
fn label(labels: BTreeMap<String, usize>) -> String {
    if let Some(core) = labels.keys().find(|l| l.contains("/core")) {
        return core.clone();
    }
    let top = labels
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(l, _)| l.clone())
        .unwrap_or_default();
    let names: BTreeSet<&str> = labels
        .keys()
        .map(|l| l.split(' ').next().unwrap_or(l))
        .collect();
    match top.split_once('/') {
        Some((scope, _)) if top.starts_with('@') && names.len() > 1 => format!("{scope}/*"),
        _ => top,
    }
}

/// (rótulo por versão → contagem, repos, palavras)
type Acc = (BTreeMap<String, usize>, Vec<String>, Vec<String>);

/// Varre os manifestos de cada repo (só leitura).
/// ponytail: sem cache por mtime, ler ~50 `package.json` e `Dockerfile` leva milissegundos; cache se os repos crescerem muito
pub fn profile(root: &Path, notes: &[Note]) -> Profile {
    // chave → (label por versão → contagem, repos)
    let mut by: BTreeMap<String, Acc> = BTreeMap::new();
    let mut add = |key: &str, label: String, keys: Vec<String>, repo: &str| {
        let e = by.entry(key.to_string()).or_default();
        *e.0.entry(label).or_default() += 1;
        if !e.1.iter().any(|r| r == repo) {
            e.1.push(repo.to_string());
        }
        if e.2.is_empty() {
            e.2 = keys;
        }
    };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists())
        .collect();
    dirs.sort();
    // (repo, pacote, versão, chave)
    let mut npm: Vec<(String, String, Option<String>, String)> = Vec::new();
    for dir in &dirs {
        let repo = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let pkg = std::fs::read_to_string(dir.join("package.json")).unwrap_or_default();
        let Ok(pkg) = serde_json::from_str::<Value>(&pkg) else {
            continue;
        };
        for sec in ["dependencies", "devDependencies"] {
            for (name, ver) in pkg[sec].as_object().into_iter().flatten() {
                if let Some(key) = npm_key(name) {
                    npm.push((
                        repo.clone(),
                        name.clone(),
                        ver.as_str().and_then(major),
                        key,
                    ));
                }
            }
        }
    }
    let keys: BTreeSet<String> = npm.iter().map(|x| x.3.clone()).collect();
    for (repo, name, ver, key) in &npm {
        let key = family(key, &keys);
        let label = match ver {
            Some(m) => format!("{name} {m}"),
            None => name.clone(),
        };
        add(&format!("npm:{key}"), label, vec![key.clone()], repo);
    }
    for dir in &dirs {
        let repo = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        for line in read("Dockerfile").lines() {
            let Some(img) = line.trim().strip_prefix("FROM ") else {
                continue;
            };
            let img = img.split_whitespace().next().unwrap_or_default();
            let name = img.rsplit('/').next().unwrap_or(img);
            let (base, tag) = name.split_once(':').unwrap_or((name, ""));
            let ver: String = tag
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>()
                .trim_end_matches('.')
                .into();
            let keys = match base {
                "node" => vec!["node.js".into(), "nodejs".into(), "node".into()],
                "python" => vec!["python".into()],
                "golang" => vec!["golang".into(), "go".into()],
                _ => continue,
            };
            let label = if ver.is_empty() {
                base.to_string()
            } else {
                format!("{base}:{ver}")
            };
            add(&format!("img:{base}"), label, keys, &repo);
        }
        let ci = read(".gitlab-ci.yml").to_lowercase();
        if !ci.is_empty() {
            add(
                "ci:gitlab",
                "GitLab CI".into(),
                vec!["gitlab".into()],
                &repo,
            );
            if ["cloud-run", "cloudrun", "gcloud run"]
                .iter()
                .any(|k| ci.contains(k))
            {
                add(
                    "ci:run",
                    "Cloud Run".into(),
                    vec!["cloud run".into(), "cloud-run".into()],
                    &repo,
                );
            }
            if ["k8s", "kubectl", "helm", "kubernetes"]
                .iter()
                .any(|k| ci.contains(k))
            {
                add(
                    "ci:k8s",
                    "k8s".into(),
                    vec!["kubernetes".into(), "k8s".into()],
                    &repo,
                );
            }
        }
    }
    let min = if dirs.len() >= 6 { 3 } else { 1 };
    let mut deps: Vec<Dep> = by
        .into_iter()
        .filter(|(k, (_, repos, _))| !k.starts_with("npm:") || repos.len() >= min)
        .map(|(k, (labels, repos, keys))| Dep {
            label: label(labels),
            keys,
            repos,
            infra: k.starts_with("ci:"),
        })
        .collect();
    deps.sort_by(|a, b| {
        b.repos
            .len()
            .cmp(&a.repos.len())
            .then(a.label.cmp(&b.label))
    });
    let names = |d: &str| {
        notes
            .iter()
            .filter(|n| n.dir == d)
            .map(|n| n.name.clone())
            .collect()
    };
    Profile {
        deps,
        decisions: names("decisions"),
        repo_notes: names("repos"),
    }
}

// ---------- pontuação ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub post: Post,
    pub score: f32,
    /// repos que a notícia pode afetar (dep sua + palavra de risco)
    pub aff: Vec<String>,
    pub dep: Option<String>,
    pub vault: Option<String>,
    pub why: String,
}

/// `key` como palavra inteira em `text` (já em minúsculas).
fn has_word(text: &str, key: &str) -> bool {
    text.match_indices(key).any(|(i, _)| {
        let ok = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        ok(text[..i].chars().next_back()) && ok(text[i + key.len()..].chars().next())
    })
}

const RISK: &[(&str, &str)] = &[
    ("breaking", "breaking change"),
    ("deprecat", "deprecação"),
    ("cve-", "falha de segurança"),
    ("vulnerab", "falha de segurança"),
    ("security", "falha de segurança"),
    ("segurança", "falha de segurança"),
    ("end of life", "fim de suporte"),
    ("end-of-life", "fim de suporte"),
    ("eol", "fim de suporte"),
    ("supply chain", "ataque de supply chain"),
    ("malicious", "pacote malicioso"),
    ("compromised", "pacote comprometido"),
    ("migrat", "migração"),
    // só com versão no título: "release" sozinho é quase todo post
    ("release", "versão nova"),
];

const STOP: &[&str] = &[
    "para", "pelo", "pela", "como", "with", "from", "that", "this", "the",
];

fn hit(p: &Post, text: &str, d: &Dep) -> bool {
    d.keys
        .iter()
        .any(|k| p.tags.iter().any(|t| t == k) || has_word(text, k))
}

/// `node:24` → `24`; `@nestjs/core 10` → `10`; `python:3.13` → `3.13` (o 3.16 alpha não é o seu)
fn major_of(label: &str) -> Option<String> {
    let v = label.rsplit([' ', ':']).next()?;
    let m: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!m.is_empty() && v != label).then_some(m)
}

/// Notícia velha não é notícia: a busca do daily.dev devolve posts de meses atrás.
const MAX_AGE: u64 = 30 * 86400;

pub fn score(posts: &[Post], prof: &Profile, foryou: &BTreeSet<String>, now: u64) -> Vec<Scored> {
    let mut out: Vec<Scored> = Vec::new();
    for p in posts {
        if out.iter().any(|s| s.post.id == p.id)
            || (p.published > 0 && now.saturating_sub(p.published) > MAX_AGE)
        {
            continue;
        }
        let title = p.title.to_lowercase();
        let text = format!("{} {}", p.title, p.summary).to_lowercase();
        let hits: Vec<&Dep> = prof.deps.iter().filter(|d| hit(p, &text, d)).collect();
        // risco só conta no título: newsletter cita "security" de passagem no resumo
        let versioned = p.title.chars().any(|c| c.is_ascii_digit());
        let mut risk = RISK
            .iter()
            .find(|(w, _)| title.contains(w) && (*w != "release" || versioned))
            .map(|(_, what)| *what);
        // a dep no título com o major que você usa: "Node.js 24.22" para quem está em node:24
        let bump = hits.iter().find(|d| {
            !d.infra
                && d.keys.iter().any(|k| has_word(&title, k))
                && major_of(&d.label).is_some_and(|m| has_word(&title, &m))
        });
        if risk.is_none() && bump.is_some() {
            risk = Some("versão nova");
        }
        let mut s: f32 = hits
            .iter()
            .map(|d| 0.15 + 0.02 * d.repos.len().min(15) as f32)
            .sum::<f32>()
            .min(0.6);
        if foryou.contains(&p.id) {
            s += 0.1;
        }
        let vault = prof.decisions.iter().find(|n| {
            n.split('-')
                .filter(|w| {
                    (w.len() >= 4 || w.chars().all(|c| c.is_ascii_digit())) && !STOP.contains(w)
                })
                .filter(|w| has_word(&text, w))
                .count()
                >= 2
        });
        if vault.is_some() {
            s += 0.15;
        }
        // "pode afetar" só com a dependência no título: nos tags e no resumo ela aparece de passagem
        let in_title = |d: &Dep| d.keys.iter().any(|k| has_word(&title, k));
        let top = bump.or_else(|| {
            hits.iter()
                .max_by_key(|d| (!d.infra, in_title(d), d.repos.len()))
        });
        let aff = match (risk, top.filter(|d| !d.infra && in_title(d))) {
            (Some(_), Some(d)) => {
                s += 0.4;
                d.repos.clone()
            }
            _ => Vec::new(),
        };
        if s < MIN_SCORE {
            continue;
        }
        let why = match (risk, top) {
            (Some(r), Some(d)) if d.infra => format!("{r} · {} ({} repos)", d.label, d.repos.len()),
            (Some("versão nova"), Some(d)) => {
                format!("versão nova de {} ({} repos)", d.label, d.repos.len())
            }
            (Some(r), Some(d)) => format!(
                "{r} numa dependência de {} repo{} seu{}",
                d.repos.len(),
                if d.repos.len() == 1 { "" } else { "s" },
                if d.repos.len() == 1 { "" } else { "s" }
            ),
            (None, Some(d)) => format!("{} · {} repos", d.label, d.repos.len()),
            _ => "no seu For You".into(),
        };
        out.push(Scored {
            post: p.clone(),
            score: s,
            aff,
            dep: top.map(|d| d.label.clone()),
            vault: vault.cloned(),
            why,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

// ---------- o que fica só aqui ----------

/// a reordenação do dia pelo Claude: (data, [(id, motivo)], tokens)
pub type Rank = (String, Vec<(String, String)>, u64);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    pub fetched_at: u64,
    pub posts: Vec<Post>,
    pub foryou: BTreeSet<String>,
    /// palavra do perfil → tag do daily.dev (`None`: não existe)
    pub tags: BTreeMap<String, Option<String>>,
    pub rank: Option<Rank>,
    pub read: BTreeSet<String>,
    pub bookmarks: BTreeSet<String>,
}

impl Cache {
    pub fn load() -> Self {
        std::fs::read_to_string(cache_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        save(
            &cache_file(),
            &serde_json::to_string(self).unwrap_or_default(),
        );
    }

    pub fn stale(&self, now: u64) -> bool {
        now.saturating_sub(self.fetched_at) >= EVERY
    }
}

fn save(path: &Path, text: &str) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, text);
}

/// "Não relevante": (tag, fonte) só local, sem escrever no daily.dev.
pub fn ignored() -> Vec<(String, String)> {
    std::fs::read_to_string(ignore_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn set_ignored(list: &[(String, String)]) {
    save(
        &ignore_file(),
        &serde_json::to_string(list).unwrap_or_default(),
    );
}

pub fn is_ignored(p: &Post, list: &[(String, String)]) -> bool {
    list.iter()
        .any(|(t, s)| *s == p.source && p.tags.iter().any(|x| x == t))
}

// ---------- API ----------

/// Separa o corpo do `-w '\n%{http_code} %header{retry-after}'` do curl.
pub fn split_status(out: &str) -> (&str, u16, u64) {
    let (body, tail) = out.rsplit_once('\n').unwrap_or(("", out));
    let mut it = tail.split_whitespace();
    let code = it.next().and_then(|c| c.parse().ok()).unwrap_or(0);
    let retry = it.next().and_then(|c| c.parse().ok()).unwrap_or(0);
    (body, code, retry)
}

/// O token vai pelo stdin (`-H @-`): nunca aparece no `ps`.
fn curl(method: &str, path: &str, body: Option<&Value>) -> Result<(String, u16, u64), String> {
    let token = std::fs::read_to_string(token_file()).map_err(|_| "sem token".to_string())?;
    let curl = std::env::var("MAESTRO_CURL").unwrap_or_else(|_| "curl".into());
    let mut c = Command::new(curl);
    c.args(["-sS", "--max-time", "20", "-X", method, "-H", "@-"])
        .args(["-w", "\n%{http_code} %header{retry-after}"]);
    if let Some(b) = body {
        c.args(["-H", "Content-Type: application/json", "--data-raw"])
            .arg(b.to_string());
    }
    let mut child = c
        .arg(format!("{API}{path}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        let _ = writeln!(s, "Authorization: Bearer {}", token.trim());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (b, code, retry) = split_status(&text);
    Ok((b.to_string(), code, retry))
}

pub fn api(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let mut tries = 0;
    loop {
        let (b, code, retry) = curl(method, path, body)?;
        match code {
            200..=299 => return Ok(serde_json::from_str(&b).unwrap_or(Value::Null)),
            429 if tries == 0 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_secs(retry.clamp(1, 60)));
            }
            0 => return Err("daily.dev fora do ar ou sem rede".into()),
            401 | 403 => return Err(format!("daily.dev recusou o token ({code})")),
            _ => return Err(format!("daily.dev respondeu {code}")),
        }
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn posts_of(v: &Value) -> Vec<Post> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(post)
        .collect()
}

/// For You + os feeds dos tags que mais aparecem nos repos + busca pelas deps mais usadas.
pub fn collect(prof: &Profile, cache: &mut Cache) -> Result<(), String> {
    let foryou = posts_of(&api("GET", "/feeds/foryou?limit=50", None)?);
    cache.foryou = foryou.iter().map(|p| p.id.clone()).collect();
    let mut posts = foryou;
    let mut feeds = 0;
    for d in prof.deps.iter().take(14) {
        if feeds >= 10 {
            break;
        }
        let mut tag = None;
        for key in &d.keys {
            let t = match cache.tags.get(key) {
                Some(t) => t.clone(),
                None => {
                    let r = api("GET", &format!("/search/tags?q={}", enc(key)), None)?;
                    let want = key.replace(['.', ' ', '-'], "");
                    let t = r["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|t| t["name"].as_str())
                        .find(|n| n.replace('-', "") == want)
                        .map(String::from);
                    cache.tags.insert(key.clone(), t.clone());
                    t
                }
            };
            if t.is_some() {
                tag = t;
                break;
            }
        }
        if let Some(t) = tag {
            feeds += 1;
            posts.extend(posts_of(&api(
                "GET",
                &format!("/feeds/tag/{}?limit=20", enc(&t)),
                None,
            )?));
        }
    }
    for d in prof
        .deps
        .iter()
        .filter(|d| d.label.starts_with('@') || !d.label.contains(' '))
        .take(3)
    {
        if let Some(k) = d.keys.first() {
            posts.extend(posts_of(&api(
                "GET",
                &format!("/search/posts?q={}&time=week&limit=10", enc(k)),
                None,
            )?));
        }
    }
    let mut seen = BTreeSet::new();
    posts.retain(|p| seen.insert(p.id.clone()));
    cache.posts = posts;
    cache.fetched_at = now();
    Ok(())
}

pub fn bookmark(id: &str, on: bool) -> Result<(), String> {
    if on {
        api("POST", "/bookmarks/", Some(&json!({ "postIds": [id] }))).map(drop)
    } else {
        api("DELETE", &format!("/bookmarks/{}", enc(id)), None).map(drop)
    }
}

/// Uma reordenação por dia, com o Claude mais barato e sem ferramentas: (ids na ordem, motivo), tokens.
pub fn rerank(prof: &Profile, items: &[Scored]) -> Result<(Vec<(String, String)>, u64), String> {
    let schema = r#"{"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"why":{"type":"string"}},"required":["id","why"]}}},"required":["items"]}"#;
    let perfil: Vec<String> = prof
        .deps
        .iter()
        .take(15)
        .map(|d| format!("{} ({} repos)", d.label, d.repos.len()))
        .collect();
    let lista: Vec<String> = items
        .iter()
        .take(30)
        .map(|s| {
            format!(
                "{} | {} | {}",
                s.post.id,
                s.post.title,
                s.post.summary.chars().take(200).collect::<String>()
            )
        })
        .collect();
    let prompt = format!(
        "Sou dev backend (Node/TypeScript, NestJS, GCP, GitLab). Meu stack: {}.\n\
         Ordene as notícias abaixo da mais para a menos relevante para o meu trabalho e descarte as irrelevantes.\n\
         Para cada uma que ficar, um motivo curto em português (até 12 palavras).\n\n{}",
        perfil.join(", "),
        lista.join("\n")
    );
    let mut child = Command::new("claude")
        .args([
            "-p",
            "--model",
            "haiku",
            "--tools",
            "",
            "--output-format",
            "json",
            "--json-schema",
            schema,
            "--max-budget-usd",
            "0.05",
            "--no-session-persistence",
            "--strict-mcp-config",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("claude: {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(prompt.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let u = &v["usage"];
    let tokens = [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .filter_map(|k| u[k].as_u64())
    .sum();
    let list = v["structured_output"]["items"]
        .as_array()
        .ok_or("o Claude não devolveu a lista")?
        .iter()
        .filter_map(|i| {
            Some((
                i["id"].as_str()?.to_string(),
                i["why"].as_str()?.to_string(),
            ))
        })
        .collect();
    Ok((list, tokens))
}

/// A ordem final: o que o Claude escolheu hoje primeiro (com o motivo dele), o resto pela pontuação.
pub fn ordered(mut xs: Vec<Scored>, rank: Option<&[(String, String)]>) -> Vec<Scored> {
    let Some(rank) = rank.filter(|r| !r.is_empty()) else {
        return xs;
    };
    let pos = |id: &str| rank.iter().position(|(r, _)| r == id);
    for s in &mut xs {
        if let Some(i) = pos(&s.post.id) {
            s.why = rank[i].1.clone();
        }
    }
    xs.sort_by_key(|s| pos(&s.post.id).unwrap_or(usize::MAX));
    xs
}

// ---------- vault ----------

/// A daily de hoje com `- [ ] ler: [título](url)` no fim da Fila (do template se ainda não existe).
pub fn with_link(daily: Option<&str>, date: &str, title: &str, url: &str) -> String {
    let task = format!("- [ ] ler: [{}]({url})", title.replace(['[', ']'], ""));
    let text = daily.map(String::from).unwrap_or_else(|| {
        let tpl = std::fs::read_to_string(crate::vault::dir().join("templates/daily.md"))
            .unwrap_or_else(|_| "# {{date:YYYY-MM-DD}}\n\n## Fila\n- [ ] \n\n## Notas\n".into());
        tpl.replace("{{date:YYYY-MM-DD}}", date)
    });
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let Some(h) = lines.iter().position(|l| l.trim() == "## Fila") else {
        lines.extend(["".into(), "## Fila".into(), task]);
        return lines.join("\n") + "\n";
    };
    // a tarefa vazia do template vira a do link
    let end = lines[h + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map_or(lines.len(), |i| h + 1 + i);
    if let Some(i) = (h + 1..end).find(|&i| lines[i].trim() == "- [ ]") {
        lines[i] = task;
    } else {
        let last = (h + 1..end)
            .rev()
            .find(|&i| !lines[i].trim().is_empty())
            .unwrap_or(h);
        lines.insert(last + 1, task);
    }
    lines.join("\n") + "\n"
}

/// `state.toml` (ou `MAESTRO_STATE`): o dia em que o Resumo do dia já apareceu.
fn state_file() -> PathBuf {
    env_or("MAESTRO_STATE", home().join(".config/maestro/state.toml"))
}

pub fn brief_due(today: &str) -> bool {
    !std::fs::read_to_string(state_file())
        .unwrap_or_default()
        .lines()
        .any(|l| l.trim() == format!("news_brief = \"{today}\""))
}

pub fn brief_shown(today: &str) {
    let path = state_file();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("news_brief"))
        .collect();
    let line = format!("news_brief = \"{today}\"");
    lines.push(&line);
    save(&path, &(lines.join("\n") + "\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, title: &str, tags: &[&str], src: &str) -> Post {
        Post {
            id: id.into(),
            title: title.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            source: src.into(),
            ..Default::default()
        }
    }

    fn prof() -> Profile {
        let d = |label: &str, keys: &[&str], n: usize| Dep {
            label: label.into(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            repos: (0..n).map(|i| format!("repo{i}")).collect(),
            infra: label == "GitLab CI",
        };
        Profile {
            deps: vec![
                d("node:24", &["node.js", "nodejs", "node"], 12),
                d("@nestjs/core 10", &["nestjs"], 7),
                d("jest 29", &["jest"], 6),
                d("GitLab CI", &["gitlab"], 40),
            ],
            decisions: vec!["node-24-dimensiona-a-heap-pelo-cgroup".into()],
            repo_notes: vec![],
        }
    }

    #[test]
    fn titulo_de_tweet_vira_uma_linha() {
        let mut p = Post {
            title: "\n  this is why opus 5.5 is the best\n\nit rewrote".into(),
            ..Default::default()
        };
        assert_eq!(p.headline(), "this is why opus 5.5 is the best");
        p.title = "x".repeat(150);
        assert_eq!(p.headline().chars().count(), 101);
    }

    #[test]
    fn pontua_pelo_que_voce_usa() {
        let posts = [
            p(
                "1",
                "NestJS 11 released: breaking changes in ConfigModule",
                &["nestjs"],
                "NestJS Blog",
            ),
            p(
                "2",
                "Node.js 24.17 fixes a security issue in undici",
                &["nodejs", "security"],
                "Node.js Blog",
            ),
            p("3", "How I organize my Rust workspace", &["rust"], "Medium"),
            p(
                "4",
                "Testing NestJS modules with Jest",
                &["nestjs", "jest"],
                "Medium",
            ),
            p("5", "Nodemon tips", &[], "Medium"),
            p(
                "6",
                "GitLab 19.4.2 fixes a critical security bug",
                &["gitlab"],
                "GitLab",
            ),
            p("7", "Jest release notes roundup", &["jest"], "Medium"),
            p("8", "Node.js 24.22.0 (LTS)", &["nodejs"], "Node.js"),
            Post {
                summary: "also: security fixes in express".into(),
                ..p(
                    "9",
                    "Node Weekly #644: NodeConf talks",
                    &["nodejs"],
                    "Node Weekly",
                )
            },
        ];
        let xs = score(&posts, &prof(), &BTreeSet::new(), 0);
        let ids: Vec<&str> = xs.iter().map(|s| s.post.id.as_str()).collect();
        assert!(!ids.contains(&"3"), "Rust não é do seu stack");
        assert!(!ids.contains(&"5"), "nodemon não é 'node' como palavra");
        let n2 = xs.iter().find(|s| s.post.id == "2").unwrap();
        assert_eq!(n2.aff.len(), 12);
        assert_eq!(n2.dep.as_deref(), Some("node:24"));
        assert_eq!(
            n2.vault.as_deref(),
            Some("node-24-dimensiona-a-heap-pelo-cgroup")
        );
        assert!(n2.why.contains("segurança"));
        let n1 = xs.iter().find(|s| s.post.id == "1").unwrap();
        assert!(n1.why.starts_with("breaking change"));
        let n4 = xs.iter().find(|s| s.post.id == "4").unwrap();
        assert!(n4.aff.is_empty(), "sem palavra de risco não 'afeta'");
        assert!(ids.iter().position(|i| *i == "4") > ids.iter().position(|i| *i == "2"));
        let n8 = xs.iter().find(|s| s.post.id == "8").unwrap();
        assert_eq!(
            (n8.aff.len(), n8.why.as_str()),
            (12, "versão nova de node:24 (12 repos)")
        );
        let n9 = xs.iter().find(|s| s.post.id == "9").unwrap();
        assert!(n9.aff.is_empty(), "risco só no resumo não 'afeta'");
        let gl = xs.iter().find(|s| s.post.id == "6").unwrap();
        assert!(gl.aff.is_empty(), "CVE do GitLab não 'afeta' os repos");
        assert!(gl.why.contains("GitLab CI"));
        assert!(
            !ids.contains(&"7"),
            "'release' sem versão no título não é risco, e jest sozinho fica abaixo do corte"
        );
        let ig = vec![("nestjs".to_string(), "Medium".to_string())];
        assert!(is_ignored(&posts[3], &ig) && !is_ignored(&posts[0], &ig));
        let r = ordered(
            xs.clone(),
            Some(&[("4".into(), "você testa NestJS".into())]),
        );
        assert_eq!(
            (r[0].post.id.as_str(), r[0].why.as_str()),
            ("4", "você testa NestJS")
        );
    }

    #[test]
    fn datas_e_status() {
        assert_eq!(epoch("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(epoch("2026-10-04T13:22:49.223Z"), Some(1791120169));
        assert_eq!(ago(0, 7200), "há 2 h");
        assert_eq!(ago(0, 90000), "ontem");
        assert_eq!(split_status("{\"a\":1}\n429 12"), ("{\"a\":1}", 429, 12));
        assert_eq!(split_status("{}\n200 "), ("{}", 200, 0));
        assert_eq!(npm_key("@nestjs/core").as_deref(), Some("nestjs"));
        assert_eq!(npm_key("@google-cloud/pubsub").as_deref(), Some("pubsub"));
        assert_eq!(npm_key("@types/node"), None);
        assert_eq!(npm_key("eslint-config-x"), None);
        assert_eq!(major("^10.2.0").as_deref(), Some("10"));
    }

    #[test]
    fn link_na_daily() {
        let t = with_link(
            Some("# d\n\n## Fila\n- [ ] \n\n## Notas\n"),
            "2026-10-15",
            "A [b]",
            "https://x",
        );
        assert_eq!(
            t,
            "# d\n\n## Fila\n- [ ] ler: [A b](https://x)\n\n## Notas\n"
        );
        let t2 = with_link(Some(&t), "2026-10-15", "C", "u");
        assert!(t2.contains("(https://x)\n- [ ] ler: [C](u)\n\n## Notas"));
        assert!(with_link(Some("# d\n"), "d", "C", "u").ends_with("## Fila\n- [ ] ler: [C](u)\n"));
    }
}
