use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

/// Custo e contexto de uma sessão do Claude Code, lidos do JSONL que ele mesmo grava.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub cost: f64,
    pub today: f64,
    pub ctx: u8,
}

#[derive(Default)]
struct Tracked {
    offset: u64,
    cost: f64,
    cost_before_today: f64,
    last_day: String,
    tokens: u64,
    one_million: bool,
}

/// Lê só o que foi acrescentado desde a última vez em cada arquivo.
#[derive(Default)]
pub struct Sessions {
    files: HashMap<PathBuf, Tracked>,
    by_pane: HashMap<String, PathBuf>,
}

/// `~/.claude/projects/<cwd com tudo que não é letra ou dígito trocado por ->`
pub fn project_dir(home: &Path, cwd: &str) -> PathBuf {
    let name: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(name)
}

impl Sessions {
    /// A sessão do pane: o JSONL do cwd cujo título (`agentName`/`aiTitle`) é o título do terminal;
    /// sem título igual, a mais recente do cwd.
    fn find(&mut self, home: &Path, pane: &str, cwd: &str, title: &str) -> Option<PathBuf> {
        if let Some(p) = self.by_pane.get(pane) {
            return Some(p.clone());
        }
        let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(project_dir(home, cwd))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
            .collect();
        files.sort_by_key(|a| std::cmp::Reverse(a.0));
        let needles = [
            format!("\"agentName\":\"{title}\""),
            format!("\"aiTitle\":\"{title}\""),
        ];
        let found = files
            .iter()
            .find(|(_, p)| {
                std::fs::read_to_string(p).is_ok_and(|t| needles.iter().any(|n| t.contains(n)))
            })
            .or(files.first())
            .map(|(_, p)| p.clone())?;
        self.by_pane.insert(pane.into(), found.clone());
        Some(found)
    }

    pub fn usage(&mut self, home: &Path, pane: &str, cwd: &str, title: &str) -> Option<Usage> {
        let path = self.find(home, pane, cwd, title)?;
        let today = today();
        let t = self.files.entry(path.clone()).or_default();
        let mut file = File::open(&path).ok()?;
        file.seek(SeekFrom::Start(t.offset)).ok()?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        // só linhas completas: o Claude pode estar no meio de uma escrita
        while reader.read_line(&mut line).ok()? > 0 && line.ends_with('\n') {
            t.offset += line.len() as u64;
            read_line(t, &line);
            line.clear();
        }
        let window = if t.one_million || t.tokens > 200_000 {
            1_000_000
        } else {
            200_000
        };
        let today_cost = if t.last_day == today {
            t.cost - t.cost_before_today
        } else {
            0.
        };
        Some(Usage {
            cost: t.cost,
            today: today_cost.max(0.),
            ctx: (t.tokens * 100 / window).min(100) as u8,
        })
    }
}

fn read_line(t: &mut Tracked, line: &str) {
    let cost = line.contains("\"type\":\"cost-state\"");
    let assistant = line.contains("\"type\":\"assistant\"");
    if let Some(i) = line.find("\"timestamp\":\"") {
        let day = &line[i + 13..(i + 23).min(line.len())];
        if day != t.last_day {
            t.cost_before_today = t.cost;
            t.last_day = day.into();
        }
    }
    if !cost && !assistant {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if cost {
        t.cost = v["totalCostUSD"].as_f64().unwrap_or(t.cost);
        t.one_million = v["modelUsage"]
            .as_object()
            .is_some_and(|m| m.keys().any(|k| k.ends_with("[1m]")));
    } else {
        let u = &v["message"]["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let tokens =
            n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        if tokens > 0 {
            t.tokens = tokens;
        }
    }
}

/// Data UTC de hoje como `AAAA-MM-DD` (os timestamps do JSONL são UTC).
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    civil(days)
}

/// Dias desde 1970-01-01 → data (algoritmo de Howard Hinnant).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datas() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_741), "2026-10-15");
    }

    #[test]
    fn le_custo_contexto_e_o_de_hoje() {
        let home = std::env::temp_dir().join(format!("maestro-sess-{}", std::process::id()));
        let dir = project_dir(&home, "/repo/x.y");
        std::fs::create_dir_all(&dir).unwrap();
        let today = today();
        let lines = [
            r#"{"type":"agent-name","agentName":"plano"}"#.to_string(),
            r#"{"type":"user","timestamp":"2020-01-01T10:00:00Z"}"#.into(),
            r#"{"type":"cost-state","totalCostUSD":10.0,"modelUsage":{"claude-x":{}}}"#.into(),
            format!(r#"{{"type":"user","timestamp":"{today}T10:00:00Z"}}"#),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":2,"cache_read_input_tokens":299998,"cache_creation_input_tokens":0}}}"#.into(),
            r#"{"type":"cost-state","totalCostUSD":12.5,"modelUsage":{"claude-x[1m]":{}}}"#.into(),
        ];
        std::fs::write(dir.join("a.jsonl"), lines.join("\n") + "\n").unwrap();
        let mut s = Sessions::default();
        let u = s.usage(&home, "p1", "/repo/x.y", "plano").unwrap();
        assert_eq!(
            u,
            Usage {
                cost: 12.5,
                today: 2.5,
                ctx: 30
            }
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
