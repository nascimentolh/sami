//! Bancos: os túneis (`cloud-sql-proxy`, `kubectl port-forward`) que já são aliases no shell do usuário
//! (no Windows, as funções e aliases do perfil do pwsh). O shell continua sendo a fonte; aqui só se lê,
//! liga e desliga.

use std::{
    collections::HashMap,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Tunnel {
    pub name: String,
    pub cmd: String,
    pub port: u16,
    /// `projeto:região:instância` do proxy (ou o alvo do port-forward)
    pub target: String,
}

impl Tunnel {
    pub fn env(&self) -> &'static str {
        let n = self.name.to_lowercase();
        if n.contains("prod") || n.contains("prd") {
            "prd"
        } else if n.contains("func") {
            "func"
        } else {
            "dev"
        }
    }

    pub fn log(&self) -> PathBuf {
        log_dir().join(format!("{}.log", self.name))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Off,
    Starting,
    Up,
    /// subiu pelo Maestro e saiu: a última linha do log
    Down(String),
}

/// Grupo de processos próprio: o túnel é do PC, fechar o Maestro não derruba.
#[cfg(unix)]
fn own_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn own_group(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

fn log_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/sql-proxy")
}

/// `nome\tvalor` por alias: o zsh imprime cru, sem as aspas do `alias`.
#[cfg(not(windows))]
const LIST: &str = r#"for k v in ${(kv)aliases}; do print -r -- "$k"$'\t'"$v"; done"#;

/// No pwsh o túnel com argumentos é uma função do perfil: o corpo dela é o comando (numa linha).
#[cfg(windows)]
const LIST: &str = r#"Get-Command -CommandType Alias,Function | ForEach-Object { "$($_.Name)`t$($_.Definition -replace '[\r\n]+',' ')" }"#;

/// Lê os aliases do shell do usuário (~0,5 s, com o perfil): rodar fora da thread da UI.
pub fn load() -> Vec<Tunnel> {
    crate::sh::capped(&mut crate::sh::login(LIST), 20)
        .ok()
        .flatten()
        .map(|o| parse(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

pub fn parse(text: &str) -> Vec<Tunnel> {
    let mut xs: Vec<Tunnel> = text
        .lines()
        .filter_map(|l| {
            // o pwsh termina a linha com \r
            let (name, cmd) = l.trim_end().split_once('\t')?;
            if !cmd.contains("cloud-sql-proxy") && !cmd.contains("port-forward") {
                return None;
            }
            let words: Vec<&str> = cmd.split_whitespace().collect();
            let port = port_of(&words)?;
            let target = words
                .iter()
                .find(|w| !w.starts_with('-') && w.matches(':').count() == 2)
                .or_else(|| words.iter().find(|w| w.contains('/')))
                .map_or_else(String::new, |w| w.to_string());
            Some(Tunnel {
                name: name.into(),
                cmd: cmd.into(),
                port,
                target,
            })
        })
        .collect();
    xs.sort_by(|a, b| a.name.cmp(&b.name));
    xs
}

fn port_of(words: &[&str]) -> Option<u16> {
    for (i, w) in words.iter().enumerate() {
        if let Some(p) = w.strip_prefix("--port=") {
            return p.parse().ok();
        }
        if matches!(*w, "--port" | "-p") {
            return words.get(i + 1)?.parse().ok();
        }
    }
    // port-forward: `5433:5432`
    words.iter().find_map(|w| {
        w.split_once(':')
            .and_then(|(a, b)| b.parse::<u16>().ok().and(a.parse().ok()))
    })
}

pub fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(150),
    )
    .is_ok()
}

/// O que o Maestro subiu: `None` rodando, `Some(())` já saiu.
#[derive(Clone, Default)]
pub struct Procs(Arc<Mutex<HashMap<String, Option<()>>>>);

impl Procs {
    pub fn state(&self, t: &Tunnel, up: bool) -> State {
        if up {
            return State::Up;
        }
        match self.0.lock().unwrap().get(&t.name) {
            Some(None) => State::Starting,
            Some(Some(())) => State::Down(last_line(t)),
            None => State::Off,
        }
    }

    /// Grupo de processos próprio: o túnel é do PC, fechar o Maestro não derruba.
    pub fn start(&self, t: &Tunnel) -> Result<(), String> {
        std::fs::create_dir_all(log_dir()).map_err(|e| e.to_string())?;
        let log = std::fs::File::create(t.log()).map_err(|e| e.to_string())?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        if let Some(aviso) = crate::sh::missing() {
            return Err(aviso.into());
        }
        // com o perfil: o corpo da função/alias pode chamar outras coisas que vivem lá
        let mut cmd = crate::sh::login(&t.cmd);
        own_group(&mut cmd);
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("{}: {e}", crate::sh::SHELL))?;
        self.0.lock().unwrap().insert(t.name.clone(), None);
        let (procs, name) = (self.0.clone(), t.name.clone());
        std::thread::spawn(move || {
            let _ = child.wait();
            if let Some(s) = procs.lock().unwrap().get_mut(&name) {
                *s = Some(());
            }
        });
        Ok(())
    }

    /// Mata quem escuta na porta (vale para o túnel aberto pelo `sql_up` no terminal também).
    pub fn stop(&self, t: &Tunnel) -> Result<(), String> {
        self.0.lock().unwrap().remove(&t.name);
        let pids = listeners(t.port)?;
        if pids.is_empty() {
            return Err(format!("nenhum processo seu escutando em :{}", t.port));
        }
        for p in pids {
            let _ = kill(p);
        }
        Ok(())
    }
}

/// Os PIDs que escutam na porta.
#[cfg(not(windows))]
fn listeners(port: u16) -> Result<Vec<u32>, String> {
    let out = Command::new("ss")
        .args(["-ltnpH", &format!("sport = :{port}")])
        .output()
        .map_err(|e| format!("ss: {e}"))?;
    Ok(pids(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(windows)]
fn listeners(port: u16) -> Result<Vec<u32>, String> {
    let line = format!(
        "(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue).OwningProcess"
    );
    let out = crate::sh::capped(&mut crate::sh::cmd(&line), 20)
        .map_err(|e| format!("Get-NetTCPConnection: {e}"))?
        .ok_or("Get-NetTCPConnection: passou do tempo")?;
    let mut v: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    v.dedup();
    Ok(v)
}

#[cfg(not(windows))]
fn kill(pid: u32) -> std::io::Result<std::process::ExitStatus> {
    Command::new("kill").arg(pid.to_string()).status()
}

#[cfg(windows)]
fn kill(pid: u32) -> std::io::Result<std::process::ExitStatus> {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
}

#[cfg(not(windows))]
fn pids(ss: &str) -> Vec<u32> {
    let mut v: Vec<u32> = ss
        .split("pid=")
        .skip(1)
        .filter_map(|s| s.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
        .collect();
    v.dedup();
    v
}

fn last_line(t: &Tunnel) -> String {
    std::fs::read_to_string(t.log())
        .ok()
        .and_then(|s| {
            s.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| "o processo saiu sem log".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_os_aliases_de_tunel() {
        let xs = parse(
            "ll\tls -alF\n\
             sql_b_prd\t~/x/cloud-sql-proxy proj-prd:us-central1:b -p 5450\n\
             sql_a_dev\t~/x/cloud-sql-proxy --address 0.0.0.0 --port 5433 proj-dev:us-central1:a\n\
             pf_redis\tkubectl port-forward -n cache svc/redis 6380:6379\n\
             sql_sem_porta\tcloud-sql-proxy p:r:i\n",
        );
        let got: Vec<_> = xs
            .iter()
            .map(|t| (t.name.as_str(), t.port, t.target.as_str(), t.env()))
            .collect();
        assert_eq!(
            got,
            [
                ("pf_redis", 6380, "svc/redis", "dev"),
                ("sql_a_dev", 5433, "proj-dev:us-central1:a", "dev"),
                ("sql_b_prd", 5450, "proj-prd:us-central1:b", "prd"),
            ]
        );
        // o `ss` (e o seu parse) só existe no Unix; no Windows quem lista é o Get-NetTCPConnection
        #[cfg(not(windows))]
        assert_eq!(
            super::pids(
                r#"LISTEN 0 4096 127.0.0.1:5433 0.0.0.0:* users:(("cloud-sql-proxy",pid=4242,fd=7),("x",pid=4242,fd=8))"#
            ),
            [4242]
        );
    }
}
