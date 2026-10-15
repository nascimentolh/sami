use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::feature;

/// Quantos pedidos do mesmo comando no mesmo repo antes de sugerir a regra.
// ponytail: conta os pedidos de permissão (o hook PermissionRequest não diz se você aprovou); 5 é o botão de ajuste
pub const LEARN_AFTER: usize = 5;
const WEEK: u64 = 7 * 24 * 3600;

fn dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/maestro")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `Bash(npm test:*)` para comandos (as duas primeiras palavras, como o `learnPerm` do protótipo); o nome da ferramenta para o resto.
pub fn rule(hook: &Value) -> Option<String> {
    let tool = hook["tool_name"].as_str()?;
    if tool != "Bash" {
        return Some(tool.to_string());
    }
    let cmd = hook["tool_input"]["command"].as_str()?;
    let key: Vec<&str> = cmd.split_whitespace().take(2).collect();
    (!key.is_empty()).then(|| format!("Bash({}:*)", key.join(" ")))
}

/// O hook `PermissionRequest` da worktree: só anota o pedido, nunca decide.
pub fn record(id: &str, stdin: &str) -> Result<(), String> {
    let hook: Value = serde_json::from_str(stdin).map_err(|e| e.to_string())?;
    let Some(rule) = rule(&hook) else {
        return Ok(());
    };
    let inv = feature::inventories()
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("feature desconhecida")?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir().join("perms.log"))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}\t{}\t{id}\t{rule}", now(), inv.repo.display()).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub repo: PathBuf,
    pub rule: String,
    pub count: usize,
    pub features: usize,
    /// a última feature que pediu: onde a regra do time entra
    pub feature: String,
}

/// Regras pedidas `LEARN_AFTER` vezes na última semana que ainda não viraram pergunta.
pub fn suggestions(log: &str, asked: &HashSet<(String, String)>, now: u64) -> Vec<Suggestion> {
    let mut seen: HashMap<(String, String), (usize, HashSet<String>, String)> = HashMap::new();
    for l in log.lines() {
        let p: Vec<&str> = l.splitn(4, '\t').collect();
        let [ts, repo, id, rule] = p[..] else {
            continue;
        };
        if ts
            .parse::<u64>()
            .is_ok_and(|t| now.saturating_sub(t) > WEEK)
        {
            continue;
        }
        let e = seen
            .entry((repo.into(), rule.into()))
            .or_insert_with(|| (0, HashSet::new(), String::new()));
        e.0 += 1;
        e.1.insert(id.into());
        e.2 = id.into();
    }
    let mut out: Vec<Suggestion> = seen
        .into_iter()
        .filter(|(k, (n, _, _))| *n >= LEARN_AFTER && !asked.contains(k))
        .map(|((repo, rule), (count, ids, feature))| Suggestion {
            repo: repo.into(),
            rule,
            count,
            features: ids.len(),
            feature,
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.count));
    out
}

fn asked_file() -> PathBuf {
    dir().join("perms.asked")
}

pub fn pending() -> Vec<Suggestion> {
    let log = std::fs::read_to_string(dir().join("perms.log")).unwrap_or_default();
    let asked = std::fs::read_to_string(asked_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    suggestions(&log, &asked, now())
}

/// Grava a resposta (qualquer uma) para a mesma pergunta não voltar.
pub fn mark(s: &Suggestion) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(asked_file())
    {
        let _ = writeln!(f, "{}\t{}", s.repo.display(), s.rule);
    }
}

fn personal_file(repo: &Path) -> PathBuf {
    dir().join(format!(
        "allow-{}.json",
        feature::slug(&repo.to_string_lossy())
    ))
}

/// As regras "só pra mim" deste repo, que entram no `settings.local.json` de cada worktree.
pub fn personal(repo: &Path) -> Vec<String> {
    std::fs::read_to_string(personal_file(repo))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn add_allow(file: &Path, rule: &str) -> Result<(), String> {
    let mut v: Value = std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let allow = v
        .as_object_mut()
        .ok_or("settings não é um objeto JSON")?
        .entry("permissions")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("permissions não é um objeto")?
        .entry("allow")
        .or_insert_with(|| json!([]));
    let list = allow.as_array_mut().ok_or("allow não é uma lista")?;
    if !list.iter().any(|r| r == rule) {
        list.push(json!(rule));
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&v).unwrap()).map_err(|e| e.to_string())
}

/// "Só pra mim": vale para as worktrees abertas deste repo e para as próximas.
pub fn allow_personal(s: &Suggestion) -> Result<(), String> {
    let mut rules = personal(&s.repo);
    if !rules.contains(&s.rule) {
        rules.push(s.rule.clone());
    }
    std::fs::write(personal_file(&s.repo), json!(rules).to_string()).map_err(|e| e.to_string())?;
    for inv in feature::inventories().iter().filter(|i| i.repo == s.repo) {
        for w in &inv.worktrees {
            add_allow(&w.path.join(".claude/settings.local.json"), &s.rule)?;
        }
    }
    Ok(())
}

/// "Para o time": o `.claude/settings.json` da worktree, que vai no próximo commit.
pub fn allow_team(s: &Suggestion) -> Result<(), String> {
    let inv = feature::inventories()
        .into_iter()
        .find(|i| i.id == s.feature)
        .ok_or("a feature que pediu já foi concluída")?;
    add_allow(
        &inv.worktrees[0].path.join(".claude/settings.json"),
        &s.rule,
    )
}

#[cfg(test)]
mod tests {
    use super::{LEARN_AFTER, rule, suggestions};
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn aprende_depois_de_n() {
        assert_eq!(
            rule(&json!({"tool_name": "Bash", "tool_input": {"command": "npm test -- --watch"}})),
            Some("Bash(npm test:*)".into())
        );
        assert_eq!(
            rule(&json!({"tool_name": "WebFetch"})),
            Some("WebFetch".into())
        );
        let line = |ts: u64, id: &str| format!("{ts}\t/r\t{id}\tBash(npm test:*)\n");
        let mut log: String = (0..LEARN_AFTER - 1)
            .map(|i| line(1000, if i % 2 == 0 { "a" } else { "b" }))
            .collect();
        assert!(suggestions(&log, &HashSet::new(), 1000).is_empty());
        log.push_str(&line(1000, "c"));
        let s = suggestions(&log, &HashSet::new(), 1000);
        assert_eq!(
            (s[0].count, s[0].features, s[0].feature.as_str()),
            (LEARN_AFTER, 3, "c")
        );
        let asked = HashSet::from([("/r".to_string(), "Bash(npm test:*)".to_string())]);
        assert!(suggestions(&log, &asked, 1000).is_empty());
        // pedido de mais de uma semana não conta
        assert!(suggestions(&log, &HashSet::new(), 1000 + 8 * 24 * 3600).is_empty());
    }
}
