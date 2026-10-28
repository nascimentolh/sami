use std::path::Path;

use serde_json::Value;

use crate::forja::Forja;

/// O `f.mr` do protótipo: o MR do GitLab ou o PR do GitHub, lido pela CLI da forja (que já tem o
/// seu login). O modelo é um só; o que muda é de onde ele vem.
#[derive(Debug, Clone, PartialEq)]
pub struct Mr {
    pub forja: Forja,
    pub iid: u64,
    pub merged: bool,
    pub closed: bool,
    pub web_url: String,
    pub title: String,
    pub target: String,
    pub pipe: Status,
    /// (nome, link) do job que falhou
    pub job: Option<(String, String)>,
    pub sonar: Status,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Ok,
    Fail,
    Running,
    /// sem pipeline, ou sem job de Sonar (SONAR_BYPASS)
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: u64,
    pub who: String,
    pub at: String,
    pub body: String,
}

impl Mr {
    /// `!7` no GitLab, `#7` no GitHub.
    pub fn etiqueta(&self) -> String {
        format!("{}{}", self.forja.marca(), self.iid)
    }

    /// `MR !7` / `PR #7`.
    pub fn titulo_curto(&self) -> String {
        format!("{} {}", self.forja.termo(), self.etiqueta())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    /// o remoto não é de GitLab nem de GitHub: só o git responde
    SemForja,
    NoAuth(Forja),
    NoMr,
    Mr(Box<Mr>),
    Err(String),
}

fn status(s: &str) -> Status {
    match s {
        "success" | "manual" | "skipped" => Status::Ok,
        "failed" | "canceled" => Status::Fail,
        "" => Status::None,
        _ => Status::Running,
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// O job que diz o quality gate: o `sonar`/`sonar-merge-request` do pipeline roda com `sonar.qualitygate.wait=true`.
pub fn sonar(jobs: &Value) -> Status {
    jobs.as_array()
        .into_iter()
        .flatten()
        .find(|j| matches!(j["name"].as_str(), Some("sonar" | "sonar-merge-request")))
        .map_or(Status::None, |j| status(j["status"].as_str().unwrap_or("")))
}

pub fn failed_job(jobs: &Value) -> Option<(String, String)> {
    jobs.as_array()?
        .iter()
        .find(|j| j["status"] == "failed" && j["allow_failure"] != true)
        .map(|j| {
            (
                j["name"].as_str().unwrap_or_default().into(),
                j["web_url"].as_str().unwrap_or_default().into(),
            )
        })
}

/// GitLab Duo e os bots de grupo/projeto (`group_…_bot_…`, como o da "Revisão automática").
fn is_bot(user: &str) -> bool {
    user == "GitLabDuo"
        || ((user.starts_with("group_") || user.starts_with("project_")) && user.contains("_bot_"))
}

/// Comentários de revisores: o primeiro de cada discussão aberta que não é do autor nem do sistema.
/// Nota solta de bot (o resumo "nada a comentar") fica de fora; o comentário de linha dele, aberto, entra.
pub fn comments(discussions: &Value, author: &str) -> Vec<Comment> {
    discussions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let n = &d["notes"][0];
            let open = n["resolvable"] != true || n["resolved"] != true;
            let who = n["author"]["username"].as_str().unwrap_or_default();
            let bot_note = is_bot(who) && n["resolvable"] != true;
            (open && !bot_note && n["system"] != true && who != author).then(|| Comment {
                id: n["id"].as_u64().unwrap_or_default(),
                who: format!("@{who}"),
                at: match n["position"]["new_path"].as_str() {
                    Some(p) => match n["position"]["new_line"].as_u64() {
                        Some(l) => format!("{p}:{l}"),
                        None => p.into(),
                    },
                    None => "geral".into(),
                },
                body: n["body"].as_str().unwrap_or_default().into(),
            })
        })
        .collect()
}

pub fn parse(detail: &Value, jobs: &Value, discussions: &Value) -> Mr {
    let state = detail["state"].as_str().unwrap_or_default();
    let pipe = status(detail["head_pipeline"]["status"].as_str().unwrap_or(""));
    Mr {
        forja: Forja::GitLab,
        iid: detail["iid"].as_u64().unwrap_or_default(),
        merged: state == "merged",
        closed: state == "closed",
        web_url: detail["web_url"].as_str().unwrap_or_default().into(),
        title: detail["title"].as_str().unwrap_or_default().into(),
        target: detail["target_branch"].as_str().unwrap_or_default().into(),
        pipe,
        job: (pipe == Status::Fail).then(|| failed_job(jobs)).flatten(),
        sonar: sonar(jobs),
        comments: comments(
            discussions,
            detail["author"]["username"].as_str().unwrap_or_default(),
        ),
    }
}

/// Uma chamada de leitura na forja, com o erro traduzido para o estado da aba.
fn api(f: Forja, root: &Path, endpoint: &str) -> Result<Value, Fetch> {
    f.read(Some(root), endpoint, false).map_err(|e| {
        // sem token, o GitLab responde 404 (não 401) para projeto privado
        let auth = e.contains("401")
            || e.contains("auth login")
            || e.contains("authenticat")
            || (e.contains("404") && !f.logado());
        match auth {
            true => Fetch::NoAuth(f),
            false => Fetch::Err(e),
        }
    })
}

/// "Mover p/ main" da pilha: o alvo do MR muda no GitLab (só depois da confirmação, pelo `mrs::write`).
pub fn retarget(m: &Mr, branch: &str) -> Result<(), String> {
    match m.forja {
        Forja::GitLab => crate::mrs::write(
            "PUT",
            &retarget_endpoint(m).ok_or("o MR não tem link do GitLab")?,
            &serde_json::json!({ "target_branch": branch }),
        )
        .map(drop),
        // `dono/repo` sai do próprio link do PR
        Forja::GitHub => {
            let proj = m
                .web_url
                .split_once(".com/")
                .and_then(|(_, r)| r.split_once("/pull/"))
                .map(|(p, _)| p)
                .ok_or("o PR não tem link do GitHub")?;
            crate::mrs::gh_write(
                "PATCH",
                &crate::github::base(proj, m.iid),
                &serde_json::json!({ "base": branch }),
            )
            .map(drop)
        }
    }
}

/// `https://gitlab.com/g/p/-/merge_requests/7` → `projects/g%2Fp/merge_requests/7`.
fn retarget_endpoint(m: &Mr) -> Option<String> {
    let path = m
        .web_url
        .split_once("://")?
        .1
        .split_once('/')?
        .1
        .split_once("/-/merge_requests/")?
        .0;
    Some(format!("projects/{}/merge_requests/{}", enc(path), m.iid))
}

/// O MR (ou PR) mais recente da branch. Só lê: o que escreve na forja são as skills.
pub fn fetch(root: &Path, branch: &str) -> Fetch {
    match Forja::de(root) {
        None => Fetch::SemForja,
        Some(Forja::GitLab) => do_gitlab(root, branch),
        Some(Forja::GitHub) => do_github(root, branch),
    }
}

fn do_gitlab(root: &Path, branch: &str) -> Fetch {
    let f = Forja::GitLab;
    let run = || -> Result<Fetch, Fetch> {
        let list = api(
            f,
            root,
            &format!(
                "projects/:id/merge_requests?source_branch={}&state=all&order_by=created_at&per_page=1",
                enc(branch)
            ),
        )?;
        let Some(iid) = list[0]["iid"].as_u64() else {
            return Ok(Fetch::NoMr);
        };
        let detail = api(f, root, &format!("projects/:id/merge_requests/{iid}"))?;
        let jobs = match detail["head_pipeline"]["id"].as_u64() {
            Some(p) => api(
                f,
                root,
                &format!("projects/:id/pipelines/{p}/jobs?per_page=100"),
            )?,
            None => Value::Null,
        };
        let discussions = api(
            f,
            root,
            &format!("projects/:id/merge_requests/{iid}/discussions?per_page=100"),
        )?;
        Ok(Fetch::Mr(Box::new(parse(&detail, &jobs, &discussions))))
    };
    run().unwrap_or_else(|e| e)
}

fn do_github(root: &Path, branch: &str) -> Fetch {
    let f = Forja::GitHub;
    // o dono/repo sai do remoto, não do `gh`: num fork ele resolveria para o repositório-mãe
    let Some(proj) = crate::forja::projeto(root) else {
        return Fetch::Err("não deu para ler dono/repo do remoto origin".into());
    };
    let dono = proj.split('/').next().unwrap_or_default().to_string();
    let run = || -> Result<Fetch, Fetch> {
        let list = api(
            f,
            root,
            &format!(
                "repos/{proj}/pulls?state=all&per_page=1&sort=created&direction=desc&head={dono}:{branch}"
            ),
        )?;
        let Some(n) = list[0]["number"].as_u64() else {
            return Ok(Fetch::NoMr);
        };
        let detail = api(f, root, &format!("repos/{proj}/pulls/{n}"))?;
        let sha = detail["head"]["sha"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // no GitHub o que diz se o CI passou são os check runs do commit do topo
        let checks = api(
            f,
            root,
            &format!("repos/{proj}/commits/{sha}/check-runs?per_page=100"),
        )
        .unwrap_or(Value::Null);
        let revisao = api(
            f,
            root,
            &format!("repos/{proj}/pulls/{n}/comments?per_page=100"),
        )?;
        let gerais = api(
            f,
            root,
            &format!("repos/{proj}/issues/{n}/comments?per_page=100"),
        )?;
        Ok(Fetch::Mr(Box::new(parse_github(
            &detail, &checks, &revisao, &gerais,
        ))))
    };
    run().unwrap_or_else(|e| e)
}

/// O mesmo modelo, vindo do GitHub: check run no lugar de job, review comment no lugar de discussão.
pub fn parse_github(detail: &Value, checks: &Value, revisao: &Value, gerais: &Value) -> Mr {
    let runs = &checks["check_runs"];
    let merged = detail["merged"] == true || detail["merged_at"].is_string();
    let autor = detail["user"]["login"].as_str().unwrap_or_default();
    Mr {
        forja: Forja::GitHub,
        iid: detail["number"].as_u64().unwrap_or_default(),
        merged,
        closed: !merged && detail["state"] == "closed",
        web_url: detail["html_url"].as_str().unwrap_or_default().into(),
        title: detail["title"].as_str().unwrap_or_default().into(),
        target: detail["base"]["ref"].as_str().unwrap_or_default().into(),
        pipe: checks_status(runs),
        job: check_falho(runs),
        sonar: sonar_github(runs),
        comments: comentarios_github(revisao, gerais, autor),
    }
}

/// Um check run: `status` diz se terminou, `conclusion` diz como.
fn check_status(c: &Value) -> Status {
    if c["status"] != "completed" {
        return Status::Running;
    }
    match c["conclusion"].as_str().unwrap_or("") {
        "success" | "skipped" | "neutral" => Status::Ok,
        "" => Status::None,
        _ => Status::Fail,
    }
}

/// O pior de todos: um falhou, falhou; algum rodando, rodando.
fn checks_status(runs: &Value) -> Status {
    let todos: Vec<Status> = runs
        .as_array()
        .into_iter()
        .flatten()
        .map(check_status)
        .collect();
    if todos.contains(&Status::Fail) {
        Status::Fail
    } else if todos.contains(&Status::Running) {
        Status::Running
    } else if todos.is_empty() {
        Status::None
    } else {
        Status::Ok
    }
}

fn sonar_github(runs: &Value) -> Status {
    runs.as_array()
        .into_iter()
        .flatten()
        .find(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| n.to_lowercase().contains("sonar"))
        })
        .map_or(Status::None, check_status)
}

fn check_falho(runs: &Value) -> Option<(String, String)> {
    runs.as_array()?
        .iter()
        .find(|c| check_status(c) == Status::Fail)
        .map(|c| {
            (
                c["name"].as_str().unwrap_or_default().into(),
                c["html_url"].as_str().unwrap_or_default().into(),
            )
        })
}

fn bot_github(u: &Value) -> bool {
    u["type"] == "Bot"
        || u["login"]
            .as_str()
            .is_some_and(|l| l.ends_with("[bot]") || l == "github-actions")
}

/// Comentários de revisores: os de linha e os gerais, sem os do autor e sem bot.
// ponytail: a REST do GitHub não diz se a conversa foi resolvida (só o GraphQL): por ora entra
// tudo que não é do autor. Se incomodar, a consulta vira GraphQL.
fn comentarios_github(revisao: &Value, gerais: &Value, autor: &str) -> Vec<Comment> {
    let de_linha = revisao.as_array().into_iter().flatten().map(|c| {
        let onde = match (
            c["path"].as_str(),
            c["line"].as_u64().or(c["original_line"].as_u64()),
        ) {
            (Some(p), Some(l)) => format!("{p}:{l}"),
            (Some(p), None) => p.to_string(),
            _ => "geral".into(),
        };
        (c, onde)
    });
    let gerais = gerais
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| (c, "geral".to_string()));
    de_linha
        .chain(gerais)
        .filter(|(c, _)| {
            let quem = c["user"]["login"].as_str().unwrap_or_default();
            quem != autor && !bot_github(&c["user"])
        })
        .map(|(c, onde)| Comment {
            id: c["id"].as_u64().unwrap_or_default(),
            who: format!("@{}", c["user"]["login"].as_str().unwrap_or_default()),
            at: onde,
            body: c["body"].as_str().unwrap_or_default().into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {

    #[test]
    fn alvo_do_mr_pelo_link() {
        let m = super::Mr {
            forja: crate::forja::Forja::GitLab,
            iid: 7,
            merged: false,
            closed: false,
            web_url: "https://gitlab.com/ctbz/experts/bots-api/-/merge_requests/7".into(),
            title: String::new(),
            target: "feat/mae".into(),
            pipe: super::Status::None,
            job: None,
            sonar: super::Status::None,
            comments: Vec::new(),
        };
        assert_eq!(
            super::retarget_endpoint(&m).as_deref(),
            Some("projects/ctbz%2Fexperts%2Fbots-api/merge_requests/7")
        );
    }

    use super::{Status, enc, parse, parse_github};
    use serde_json::json;

    /// De verdade, contra o GitHub deste repo: `cargo test --release -- --ignored pr_de_verdade`.
    /// Pede `gh auth login`, por isso fica fora da suíte normal.
    #[test]
    #[ignore]
    fn pr_de_verdade() {
        let f = super::fetch(std::path::Path::new("."), "feat/windows-pwsh");
        match f {
            super::Fetch::Mr(m) => {
                assert_eq!(m.forja, crate::forja::Forja::GitHub);
                assert_eq!(m.etiqueta(), "#1");
                assert_eq!(m.target, "main");
                assert!(m.merged, "o PR 1 foi mergeado");
                println!("{} · {} · {:?}", m.titulo_curto(), m.title, m.pipe);
            }
            outro => panic!("esperava o PR, veio {outro:?}"),
        }
    }

    #[test]
    fn le_o_pr_do_github() {
        let detail = json!({
            "number": 38, "state": "open", "merged": false,
            "title": "bump rust-cache", "html_url": "https://github.com/o/r/pull/38",
            "base": {"ref": "main"}, "head": {"sha": "cda74a24"},
            "user": {"login": "dependabot[bot]", "type": "Bot"}
        });
        let checks = json!({"check_runs": [
            {"name": "check", "status": "completed", "conclusion": "success", "html_url": "u1"},
            {"name": "sonarcloud", "status": "completed", "conclusion": "failure", "html_url": "u2"},
            {"name": "deploy", "status": "in_progress", "conclusion": null, "html_url": "u3"}
        ]});
        let revisao = json!([
            {"id": 1, "path": "src/a.ts", "line": 12, "body": "isso aqui estoura",
             "user": {"login": "revisor", "type": "User"}},
            {"id": 2, "path": "src/a.ts", "line": 3, "body": "do autor",
             "user": {"login": "dependabot[bot]", "type": "Bot"}}
        ]);
        let gerais = json!([
            {"id": 9, "body": "subi o patch", "user": {"login": "colega", "type": "User"}},
            {"id": 10, "body": "pipeline rodando", "user": {"login": "github-actions[bot]", "type": "Bot"}}
        ]);
        let m = parse_github(&detail, &checks, &revisao, &gerais);
        assert_eq!(m.etiqueta(), "#38");
        assert_eq!(m.titulo_curto(), "PR #38");
        assert_eq!(m.target, "main");
        assert!(!m.merged && !m.closed);
        // um falhou: o pior manda, mesmo com outro ainda rodando
        assert_eq!(m.pipe, Status::Fail);
        assert_eq!(m.sonar, Status::Fail);
        assert_eq!(m.job, Some(("sonarcloud".into(), "u2".into())));
        assert_eq!(
            m.comments
                .iter()
                .map(|c| (c.who.as_str(), c.at.as_str()))
                .collect::<Vec<_>>(),
            [("@revisor", "src/a.ts:12"), ("@colega", "geral")],
            "sem bot e sem o autor"
        );

        let merged = parse_github(
            &json!({"number": 2, "state": "closed", "merged_at": "2026-10-28T19:51:22Z",
                    "base": {"ref": "main"}, "user": {"login": "eu"}}),
            &json!({}),
            &json!([]),
            &json!([]),
        );
        assert!(merged.merged && !merged.closed);
        assert_eq!(merged.pipe, Status::None, "repo sem CI não é falha");
    }

    #[test]
    fn mr_com_ci_vermelho() {
        let detail = json!({"iid": 7, "state": "opened", "web_url": "u", "target_branch": "main",
            "author": {"username": "eu"}, "head_pipeline": {"id": 1, "status": "failed"}});
        let jobs = json!([
            {"name": "lint", "status": "failed", "allow_failure": true, "web_url": "l"},
            {"name": "test-vitest", "status": "failed", "allow_failure": false, "web_url": "j"},
            {"name": "sonar-project-properties-available", "status": "success"},
            {"name": "sonar-merge-request", "status": "running"}
        ]);
        let disc = json!([
            {"notes": [{"id": 1, "system": true, "author": {"username": "x"}, "body": "mudou"}]},
            {"notes": [{"id": 2, "author": {"username": "eu"}, "body": "meu"}]},
            {"notes": [{"id": 3, "author": {"username": "ana"}, "body": "ok", "resolvable": true, "resolved": true}]},
            {"notes": [{"id": 4, "author": {"username": "ana"}, "body": "timeout?", "resolvable": true, "resolved": false,
                "position": {"new_path": "src/a.ts", "new_line": 12}}]},
            {"notes": [{"id": 6, "author": {"username": "GitLabDuo"}, "body": "I finished my review", "resolvable": false}]},
            {"notes": [{"id": 7, "author": {"username": "group_1_bot_ab"}, "body": "Revisão automática", "resolvable": false}]}
        ]);
        let m = parse(&detail, &jobs, &disc);
        assert_eq!(m.pipe, Status::Fail);
        assert_eq!(m.job, Some(("test-vitest".into(), "j".into())));
        assert_eq!(m.sonar, Status::Running);
        assert_eq!(m.comments.len(), 1);
        assert_eq!(m.comments[0].at, "src/a.ts:12");
        assert_eq!(enc("feat/x y"), "feat%2Fx%20y");
    }
}
