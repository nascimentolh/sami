//! O shell em que o Maestro roda comando fora do terminal: `sh` no Unix, PowerShell 7+ no Windows.
//! O 7 é o mínimo porque o pré-voo e o setup do repo encadeiam com `&&`, que o PowerShell 5 não tem.

use std::{
    io,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
pub const SHELL: &str = "pwsh";
#[cfg(not(windows))]
pub const SHELL: &str = "sh";

/// O que mostrar no Windows sem PowerShell 7+.
pub const INSTALL: &str = "Instale o PowerShell 7+: winget install --id Microsoft.PowerShell";

/// Um comando no shell do sistema, sem o perfil de quem usa: previsível e sem o custo do rc.
pub fn cmd(line: &str) -> Command {
    let mut c = Command::new(SHELL);
    #[cfg(windows)]
    c.args(["-NoLogo", "-NoProfile", "-Command", line]);
    #[cfg(not(windows))]
    c.args(["-c", line]);
    c
}

/// Com o perfil/rc de quem usa: é lá que vivem o PATH do nvm e os aliases dos túneis.
pub fn login(line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new(SHELL);
        c.args(["-NoLogo", "-Command", line]);
        c
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| SHELL.into());
        let mut c = Command::new(shell);
        c.args(["-lic", line]);
        c
    }
}

/// Roda com teto de tempo: `None` quando passou do teto. O `timeout` do coreutils não existe no Windows.
pub fn capped(cmd: &mut Command, secs: u64) -> io::Result<Option<Output>> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // ler em thread: o pipe cheio travaria o filho antes do teto
    let mut out = child.stdout.take().expect("stdout do filho");
    let mut err = child.stderr.take().expect("stderr do filho");
    let so = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let se = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });
    let limite = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if Instant::now() >= limite {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (stdout, stderr) = (so.join().unwrap_or_default(), se.join().unwrap_or_default());
    Ok(status.map(|status| Output {
        status,
        stdout,
        stderr,
    }))
}

/// Se `bin` está no PATH (no Windows vale o `.exe`/`.cmd` do PATHEXT).
pub fn in_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    #[cfg(windows)]
    let names: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
        .split(';')
        .map(|e| format!("{bin}{}", e.to_lowercase()))
        .chain([bin.to_string()])
        .collect();
    #[cfg(not(windows))]
    let names = vec![bin.to_string()];
    std::env::split_paths(&path).any(|d| names.iter().any(|n| d.join(n).is_file()))
}

/// A major do `pwsh` da máquina (0 quando não há): uma vez só, o processo custa ~0,3 s.
#[cfg(windows)]
pub fn pwsh_major() -> u32 {
    static V: OnceLock<u32> = OnceLock::new();
    *V.get_or_init(|| {
        capped(&mut cmd("$PSVersionTable.PSVersion.Major"), 20)
            .ok()
            .flatten()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0)
    })
}

/// O pedido de instalação quando o shell que o Maestro precisa não está aí.
#[cfg(windows)]
pub fn missing() -> Option<&'static str> {
    (pwsh_major() < 7).then_some(INSTALL)
}

#[cfg(not(windows))]
pub fn missing() -> Option<&'static str> {
    None
}

/// O terminal no Windows, onde não há herdr: o pwsh 7+ de quem usa; sem ele, o PowerShell do
/// sistema abrindo com o pedido para instalar (o terminal continua servindo).
#[cfg(windows)]
pub fn launch() -> crate::terminal::Launch {
    let (program, args) = match missing() {
        None => ("pwsh", vec!["-NoLogo".to_string()]),
        Some(aviso) => (
            "powershell",
            vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                format!("Write-Host {} -ForegroundColor Yellow", ps_quote(aviso)),
            ],
        ),
    };
    crate::terminal::Launch {
        program: program.into(),
        args,
        env: Vec::new(),
        hidden_top: 0,
    }
}

/// Um literal do PowerShell: aspas simples, com a de dentro dobrada.
#[cfg(windows)]
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teto_de_tempo_mata_o_que_trava() {
        // o shell de cada plataforma dormindo mais do que o teto
        let dorme = if cfg!(windows) {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        assert!(capped(&mut cmd(dorme), 1).unwrap().is_none());
        let o = capped(&mut cmd("echo oi"), 20).unwrap().expect("terminou");
        assert!(o.status.success());
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "oi");
        assert!(in_path(if cfg!(windows) { "git" } else { "sh" }));
        assert!(!in_path("nao-existe-mesmo-123"));
    }
}
