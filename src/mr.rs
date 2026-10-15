use std::{path::Path, process::Command};

use serde_json::Value;

/// O `f.mr` do protótipo, lido do GitLab pelo `glab api` (que já tem o seu login).
#[derive(Debug, Clone, PartialEq)]
pub struct Mr {
    pub iid: u64,
    pub merged: bool,
    pub closed: bool,
    pub web_url: String,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    NoGitlab,
    NoAuth,
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

/// Comentários de revisores: o primeiro de cada discussão aberta que não é do autor nem do sistema.
pub fn comments(discussions: &Value, author: &str) -> Vec<Comment> {
    discussions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let n = &d["notes"][0];
            let open = n["resolvable"] != true || n["resolved"] != true;
            let who = n["author"]["username"].as_str().unwrap_or_default();
            (open && n["system"] != true && who != author).then(|| Comment {
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
        iid: detail["iid"].as_u64().unwrap_or_default(),
        merged: state == "merged",
        closed: state == "closed",
        web_url: detail["web_url"].as_str().unwrap_or_default().into(),
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

fn glab(root: &Path, endpoint: &str) -> Result<Value, Fetch> {
    let out = Command::new("glab")
        .args(["api", endpoint])
        .current_dir(root)
        .output()
        .map_err(|e| Fetch::Err(format!("glab: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(
            if err.contains("401") || err.contains("auth login") || err.contains("authenticat") {
                Fetch::NoAuth
            } else {
                Fetch::Err(err.lines().next().unwrap_or("glab falhou").into())
            },
        );
    }
    serde_json::from_slice(&out.stdout).map_err(|e| Fetch::Err(e.to_string()))
}

/// O MR mais recente da branch. Só lê: o que escreve no GitLab são as skills (`/mr`, `/comentario`).
pub fn fetch(root: &Path, branch: &str) -> Fetch {
    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if !remote.contains("gitlab") {
        return Fetch::NoGitlab;
    }
    let run = || -> Result<Fetch, Fetch> {
        let list = glab(
            root,
            &format!(
                "projects/:id/merge_requests?source_branch={}&state=all&order_by=created_at&per_page=1",
                enc(branch)
            ),
        )?;
        let Some(iid) = list[0]["iid"].as_u64() else {
            return Ok(Fetch::NoMr);
        };
        let detail = glab(root, &format!("projects/:id/merge_requests/{iid}"))?;
        let jobs = match detail["head_pipeline"]["id"].as_u64() {
            Some(p) => glab(
                root,
                &format!("projects/:id/pipelines/{p}/jobs?per_page=100"),
            )?,
            None => Value::Null,
        };
        let discussions = glab(
            root,
            &format!("projects/:id/merge_requests/{iid}/discussions?per_page=100"),
        )?;
        Ok(Fetch::Mr(Box::new(parse(&detail, &jobs, &discussions))))
    };
    run().unwrap_or_else(|e| e)
}

#[cfg(test)]
mod tests {
    use super::{Status, enc, parse};
    use serde_json::json;

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
                "position": {"new_path": "src/a.ts", "new_line": 12}}]}
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
