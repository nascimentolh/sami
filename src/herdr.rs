use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

/// Sessão do herdr que o Maestro usa. `MAESTRO_HERDR_SESSION=nome` isola os testes da sessão real;
/// `none` (ou herdr fora do PATH) roda só o `$SHELL`.
pub struct Herdr {
    session: Option<String>,
    socket: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Blocked,
    Working,
    Done,
    Idle,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub agent: String,
    pub agent_status: Status,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    pub cwd: String,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
    pub tab_count: u32,
    pub focused: bool,
}

impl Herdr {
    pub fn from_env() -> Option<Self> {
        let session = std::env::var("MAESTRO_HERDR_SESSION").ok();
        if session.as_deref() == Some("none") || !in_path("herdr") {
            return None;
        }
        let dir = PathBuf::from(std::env::var_os("HOME")?).join(".config/herdr");
        let socket = match &session {
            Some(name) => dir.join("sessions").join(name).join("herdr.sock"),
            None => dir.join("herdr.sock"),
        };
        Some(Self { session, socket })
    }

    pub fn command(&self) -> (String, Vec<String>) {
        let args = match &self.session {
            Some(name) => vec!["--session".into(), name.clone()],
            None => vec![],
        };
        ("herdr".into(), args)
    }

    /// Uma conexão por request: o servidor fecha o socket depois de responder.
    pub fn call(&self, method: &str, params: Value) -> io::Result<Value> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let request = json!({ "id": "maestro", "method": method, "params": params });
        writeln!(stream, "{request}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let mut reply: Value = serde_json::from_str(&line)?;
        match reply.get_mut("result") {
            Some(result) => Ok(result.take()),
            None => Err(io::Error::other(reply["error"]["message"].to_string())),
        }
    }

    pub fn agents(&self) -> io::Result<Vec<Agent>> {
        parse_agents(self.call("agent.list", json!({}))?)
    }

    pub fn workspaces(&self) -> io::Result<Vec<Workspace>> {
        let mut result = self.call("workspace.list", json!({}))?;
        Ok(serde_json::from_value(result["workspaces"].take())?)
    }

    pub fn focus_workspace(&self, id: &str) -> io::Result<()> {
        self.call("workspace.focus", json!({ "workspace_id": id }))
            .map(drop)
    }

    pub fn focus(&self, pane_id: &str) -> io::Result<()> {
        self.call("agent.focus", json!({ "target": pane_id }))
            .map(drop)
    }
}

fn parse_agents(mut result: Value) -> io::Result<Vec<Agent>> {
    Ok(serde_json::from_value(result["agents"].take())?)
}

fn in_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Raiz do repositório git que contém `cwd`, se houver.
pub fn repo_root(cwd: &Path) -> Option<&Path> {
    cwd.ancestors().find(|d| d.join(".git").exists())
}

/// Branch atual lendo `.git/HEAD` direto (worktree: `.git` é um arquivo `gitdir: …`).
pub fn branch(root: &Path) -> Option<String> {
    let dot = root.join(".git");
    let git_dir = match std::fs::read_to_string(&dot) {
        Ok(file) => root.join(file.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot,
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(name) => name.to_string(),
        None => head.get(..7)?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_o_agent_list() {
        let result = json!({"type":"agent_list","agents":[
            {"terminal_id":"t1","agent":"claude","terminal_title":"✳ plano","terminal_title_stripped":"plano",
             "agent_status":"blocked","workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p30","focused":true,
             "state_change_seq":123,"cwd":"/repos/survey-service","revision":19},
            {"terminal_id":"t2","agent":"codex","agent_status":"algo-novo","workspace_id":"w1","tab_id":"w1:t2",
             "pane_id":"w1:p4","focused":false,"cwd":"/tmp","revision":1}]});
        let agents = parse_agents(result).unwrap();
        assert_eq!(agents[0].pane_id, "w1:p30");
        assert_eq!(agents[0].agent_status, Status::Blocked);
        assert_eq!(agents[0].terminal_title_stripped.as_deref(), Some("plano"));
        assert_eq!(agents[1].agent_status, Status::Unknown);
        assert_eq!(agents[1].terminal_title_stripped, None);
    }

    #[test]
    fn le_a_branch_de_repo_e_de_worktree() {
        let dir = std::env::temp_dir().join(format!("maestro-branch-{}", std::process::id()));
        let repo = dir.join("repo");
        let wt = dir.join("wt");
        std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(repo.join(".git/worktrees/wt/HEAD"), "0123456789abcdef\n").unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(branch(&repo).as_deref(), Some("main"));
        assert_eq!(branch(&wt).as_deref(), Some("0123456"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
