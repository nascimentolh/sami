use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::terminal::Launch;
use serde_json::{Value, json};

/// Sessão do herdr que o Maestro usa: a própria `maestro`, porque a lateral e a barra de abas do herdr
/// são estado do servidor e precisam ficar escondidas sem mexer no herdr do Ghostty (sessão `default`).
/// `MAESTRO_HERDR_SESSION=nome` troca a sessão (testes: `maestro-test`); `none` roda só o `$SHELL`.
pub struct Herdr {
    session: String,
    socket: PathBuf,
}

/// O que muda na config do herdr dentro do Maestro: a lateral e as abas são desenhadas pelo Maestro.
/// A barra de abas fica sempre visível na linha 0 para o Maestro escondê-la sem depender do número de abas.
const UI_OVERRIDES: &[(&str, &str)] = &[
    ("sidebar_start_collapsed", "true"),
    ("sidebar_collapsed_mode", "\"hidden\""),
    ("hide_tab_bar_when_single_tab", "false"),
    ("tab_bar_position", "\"top\""),
];

/// A config do usuário com os `UI_OVERRIDES` no lugar dos mesmos nomes em `[ui]`.
pub fn maestro_config(user: &str) -> String {
    let overrides: String = UI_OVERRIDES
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    let mut out = String::new();
    let mut section = String::new();
    let mut placed = false;
    for line in user.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
        }
        let key = t.split('=').next().unwrap_or("").trim();
        if section == "[ui]" && UI_OVERRIDES.iter().any(|(k, _)| *k == key) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if t == "[ui]" && !placed {
            out.push_str(&overrides);
            placed = true;
        }
    }
    if !placed {
        out.push_str("\n[ui]\n");
        out.push_str(&overrides);
    }
    out
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
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    pub focused: bool,
}

impl Herdr {
    pub fn from_env() -> Option<Self> {
        let session = std::env::var("MAESTRO_HERDR_SESSION").unwrap_or_else(|_| "maestro".into());
        if session == "none" || !in_path("herdr") {
            return None;
        }
        let dir = PathBuf::from(std::env::var_os("HOME")?).join(".config/herdr");
        let socket = match session.as_str() {
            "default" => dir.join("herdr.sock"),
            name => dir.join("sessions").join(name).join("herdr.sock"),
        };
        Some(Self { session, socket })
    }

    /// Sem servidor: as chamadas falham e quem chama segue sem o herdr.
    #[cfg(test)]
    pub fn offline() -> Self {
        Self {
            session: "maestro-test-offline".into(),
            socket: "/nonexistent/herdr.sock".into(),
        }
    }

    /// O cliente herdr para o PTY, com a config do Maestro (só vale para o servidor que ele mesmo sobe).
    pub fn launch(&self) -> Launch {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let user =
            std::fs::read_to_string(home.join(".config/herdr/config.toml")).unwrap_or_default();
        let path = home.join(".config/maestro/herdr.toml");
        let written = std::fs::create_dir_all(home.join(".config/maestro"))
            .and_then(|_| std::fs::write(&path, maestro_config(&user)));
        let mut env = Vec::new();
        if written.is_ok() {
            env.push((
                "HERDR_CONFIG_PATH".into(),
                path.to_string_lossy().into_owned(),
            ));
        }
        let mut args = vec![
            "-c".into(),
            // servidor caiu (sai com erro): reabre o cliente, que sobe o servidor de novo; detach (0) fecha
            "until herdr \"$@\"; do sleep 1; done".into(),
            "herdr".into(),
        ];
        if self.session != "default" {
            args.extend(["--session".into(), self.session.clone()]);
        }
        Launch {
            program: "sh".into(),
            args,
            env,
            hidden_top: 1,
        }
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

    pub fn tabs(&self) -> io::Result<Vec<Tab>> {
        let mut result = self.call("tab.list", json!({}))?;
        Ok(serde_json::from_value(result["tabs"].take())?)
    }

    pub fn focus_tab(&self, id: &str) -> io::Result<()> {
        self.call("tab.focus", json!({ "tab_id": id })).map(drop)
    }

    pub fn new_tab(&self, workspace_id: &str) -> io::Result<()> {
        self.call(
            "tab.create",
            json!({ "workspace_id": workspace_id, "focus": true }),
        )
        .map(drop)
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

/// O repositório principal de uma worktree (`.git` é um arquivo apontando para `<repo>/.git/worktrees/…`).
pub fn main_repo(root: &Path) -> PathBuf {
    std::fs::read_to_string(root.join(".git"))
        .ok()
        .and_then(|f| {
            let dir = f.strip_prefix("gitdir:")?.trim().to_string();
            let i = dir.find("/.git/worktrees/")?;
            Some(PathBuf::from(&dir[..i]))
        })
        .unwrap_or_else(|| root.to_path_buf())
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
    fn config_do_maestro_sobrescreve_so_a_ui() {
        let user = "onboarding = false\n[keys]\nprefix = \"ctrl+a\"\n[ui]\nsidebar_collapsed_mode = \"compact\"\nprompt_new_tab_name = false\n";
        let c = maestro_config(user);
        assert!(c.contains("prefix = \"ctrl+a\""));
        assert!(c.contains("prompt_new_tab_name = false"));
        assert!(c.contains("sidebar_collapsed_mode = \"hidden\""));
        assert!(!c.contains("compact"));
        assert_eq!(c.matches("[ui]").count(), 1);
        assert!(maestro_config("").contains("[ui]\nsidebar_start_collapsed = true"));
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
        assert_eq!(main_repo(&wt), repo);
        assert_eq!(main_repo(&repo), repo);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
