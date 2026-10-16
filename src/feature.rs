use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::herdr::Herdr;

/// Tudo que a feature criou, para a conclusão e a Limpeza apagarem exatamente isso e nada mais.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub id: String,
    pub repo: PathBuf,
    pub branch: String,
    pub base: String,
    pub worktrees: Vec<Worktree>,
    pub port: u16,
    pub compose_project: String,
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// a branch foi criada pela feature (a conclusão pode apagá-la se estiver mergeada)
    pub created_branch: bool,
}

pub struct Request {
    pub repo: PathBuf,
    pub branch: String,
    pub base: String,
    pub prompt: String,
    pub copy_env: bool,
    pub setup: bool,
    pub claude: bool,
    pub model: Option<&'static str>,
    pub group: Option<String>,
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        // sem acento, como o `normalize('NFD')` do protótipo
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' => 'a',
            'é' | 'ê' | 'è' | 'É' | 'Ê' => 'e',
            'í' | 'ì' | 'Í' => 'i',
            'ó' | 'ô' | 'õ' | 'ò' | 'Ó' | 'Ô' | 'Õ' => 'o',
            'ú' | 'ü' | 'ù' | 'Ú' => 'u',
            'ç' | 'Ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// `~/repos/bots-api` + `feat/x` → `~/repos/bots-api.wt/feat-x`, como no protótipo.
pub fn worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    repo.with_file_name(format!("{name}.wt")).join(slug(branch))
}

pub fn compose_project(repo: &Path, branch: &str) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{}-{}", slug(&name), slug(branch))
}

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/share/maestro/features")
}

pub fn inventories() -> Vec<Inventory> {
    let Ok(dir) = std::fs::read_dir(data_dir()) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect()
}

pub fn save(inv: &Inventory) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::write(
        data_dir().join(format!("{}.json", inv.id)),
        serde_json::to_string_pretty(inv)?,
    )
}

/// A primeira porta acima de 3000 que nenhuma feature usa (3000 fica com a main).
pub fn next_port(used: &[u16]) -> u16 {
    (3001..).find(|p| !used.contains(p)).unwrap_or(3001)
}

/// O setup do repo: `setup = "…"` no `.maestro.toml`, senão pelo lockfile.
pub fn setup_command(repo: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(repo.join(".maestro.toml")) {
        let line = text.lines().find(|l| l.trim_start().starts_with("setup"))?;
        let v = line.split_once('=')?.1.trim().trim_matches('"');
        return (!v.is_empty()).then(|| v.to_string());
    }
    let has = |f: &str| repo.join(f).exists();
    Some(
        if has("pnpm-lock.yaml") {
            "pnpm install --frozen-lockfile"
        } else if has("yarn.lock") {
            "yarn install --frozen-lockfile"
        } else if has("package-lock.json") {
            "npm ci"
        } else {
            return None;
        }
        .into(),
    )
}

/// Os `.env*` da raiz que o git ignora (os versionados já vêm com a worktree).
pub fn env_files(repo: &Path) -> Vec<String> {
    git(
        repo,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--",
            ".env*",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|l| !l.contains('/') && !l.is_empty())
    .map(String::from)
    .collect()
}

pub fn branches(repo: &Path) -> Vec<String> {
    git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .unwrap_or_default()
    .lines()
    .map(String::from)
    .collect()
}

/// Bases do seletor "A partir de": as do protótipo que existem no repo.
pub fn bases(repo: &Path) -> Vec<String> {
    let all = branches(repo);
    let mut b: Vec<String> = ["main", "master", "develop"]
        .into_iter()
        .filter(|x| all.iter().any(|y| y == x))
        .map(String::from)
        .collect();
    if b.is_empty() {
        b.extend(all.into_iter().take(1));
    }
    b
}

/// `'…'` para o shell: o prompt vai como um argumento só, com aspas dentro.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_err(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// O comando que roda no pane da feature: setup e depois o `claude` com o prompt.
/// `context`: o arquivo com a nota do repo no vault, que vai no `--append-system-prompt`.
pub fn start_command(req: &Request, setup: Option<&str>, context: Option<&Path>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = setup.filter(|_| req.setup) {
        parts.push(s.to_string());
    }
    if req.claude {
        let mut c = String::from("claude");
        if let Some(m) = req.model {
            c.push_str(&format!(" --model {m}"));
        }
        if let Some(f) = context {
            c.push_str(&format!(
                " --append-system-prompt \"$(cat {})\"",
                sh_quote(&f.to_string_lossy())
            ));
        }
        if !req.prompt.trim().is_empty() {
            c.push(' ');
            c.push_str(&sh_quote(req.prompt.trim()));
        }
        parts.push(c);
    }
    parts.join(" && ")
}

/// Cria a worktree, copia os `.env`, grava o inventário e abre o workspace no herdr com o setup e o Claude.
pub fn create(herdr: &Herdr, req: &Request, port: u16) -> Result<Inventory, String> {
    if branches(&req.repo).contains(&req.branch) {
        return Err(format!(
            "Já existe uma feature em {}. Escolha outro nome ou abra a existente.",
            req.branch
        ));
    }
    let path = worktree_path(&req.repo, &req.branch);
    if path.exists() {
        return Err(format!("{} já existe.", path.display()));
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    git_err(
        &req.repo,
        &[
            "worktree",
            "add",
            "-b",
            &req.branch,
            &path.to_string_lossy(),
            &req.base,
        ],
    )?;
    if req.copy_env {
        for f in env_files(&req.repo) {
            let _ = std::fs::copy(req.repo.join(&f), path.join(&f));
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut inv = Inventory {
        id: format!("{}-{now}", slug(&req.branch)),
        repo: req.repo.clone(),
        branch: req.branch.clone(),
        base: req.base.clone(),
        worktrees: vec![Worktree {
            repo: req.repo.clone(),
            path: path.clone(),
            branch: req.branch.clone(),
            created_branch: true,
        }],
        port,
        compose_project: compose_project(&req.repo, &req.branch),
        workspace_id: None,
        group: req.group.clone(),
        model: req.model.map(String::from),
        created_at: now,
    };
    // grava antes do herdr: se algo falhar daqui em diante, a Limpeza ainda acha a worktree
    save(&inv).map_err(|e| e.to_string())?;

    let ws = herdr
        .call(
            "workspace.create",
            json!({
                "cwd": path,
                "label": req.branch,
                "focus": true,
                "env": { "PORT": port.to_string(), "COMPOSE_PROJECT_NAME": inv.compose_project },
            }),
        )
        .map_err(|e| format!("herdr: {e}"))?;
    let ws_id = ws["workspace"]["workspace_id"].as_str().map(String::from);
    let pane = ws["root_pane"]["pane_id"].as_str().map(String::from);
    inv.workspace_id = ws_id.clone();
    save(&inv).map_err(|e| e.to_string())?;

    let _ = install_hook(&req.repo, &path, &inv.id);
    // a nota do repo no vault e o que ela linka entram no contexto do Claude da feature
    let repo_name = req
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let context = crate::vault::context(&crate::vault::load(&crate::vault::dir()), &repo_name)
        .and_then(|c| {
            let f = data_dir().join(format!("{}.context.md", inv.id));
            std::fs::write(&f, c).ok().map(|_| f)
        });
    let cmd = start_command(req, setup_command(&req.repo).as_deref(), context.as_deref());
    if let (Some(pane), false) = (&pane, cmd.is_empty()) {
        let _ = herdr.call(
            "pane.send_text",
            json!({ "pane_id": pane, "text": format!("{cmd}\r") }),
        );
    }
    // aba 2 é zsh, como no protótipo
    if let Some(ws) = &ws_id {
        let _ = herdr.call(
            "tab.create",
            json!({ "workspace_id": ws, "cwd": path, "focus": false }),
        );
    }
    Ok(inv)
}

/// O que a confirmação precisa mostrar antes de concluir.
pub struct Pending {
    pub uncommitted: usize,
    pub merged: bool,
}

/// Conta o não commitado de todas as worktrees: as extras limpas saem sem perguntar.
pub fn pending(inv: &Inventory) -> Pending {
    let uncommitted = inv
        .worktrees
        .iter()
        .map(|w| git(&w.path, &["status", "--porcelain"]).map_or(0, |s| s.lines().count()))
        .sum();
    Pending {
        uncommitted,
        merged: is_merged(&inv.repo, &inv.branch, &inv.base),
    }
}

fn is_merged(repo: &Path, branch: &str, base: &str) -> bool {
    git(
        repo,
        &["branch", "--merged", base, "--format=%(refname:short)"],
    )
    .is_some_and(|s| s.lines().any(|l| l == branch))
}

pub fn find_by_path(path: &Path) -> Option<Inventory> {
    inventories()
        .into_iter()
        .find(|i| i.worktrees.iter().any(|w| path.starts_with(&w.path)))
}

/// Conclui a feature apagando só o que o inventário diz que ela criou. Volume do compose fica.
pub fn conclude(herdr: &Herdr, inv: &Inventory) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    if let Some(ws) = &inv.workspace_id {
        // fecha antes da worktree: o claude e o shell saem da pasta
        if herdr
            .call("workspace.close", json!({ "workspace_id": ws }))
            .is_ok()
        {
            done.push(format!("workspace {ws} fechado"));
        }
    }
    let compose = Command::new("docker")
        .args(["compose", "-p", &inv.compose_project, "down"])
        .output();
    if compose.is_ok_and(|o| o.status.success()) {
        done.push(format!("compose {} parado", inv.compose_project));
    }
    for wt in &inv.worktrees {
        crate::lsp::stop(&wt.path);
        if wt.path.exists() {
            git_err(
                &wt.repo,
                &["worktree", "remove", "--force", &wt.path.to_string_lossy()],
            )?;
            done.push(format!("worktree {} removida", wt.path.display()));
        }
        let _ = git(&wt.repo, &["worktree", "prune"]);
        if wt.created_branch && is_merged(&wt.repo, &wt.branch, &inv.base) {
            git_err(&wt.repo, &["branch", "-d", &wt.branch])?;
            done.push(format!("branch {} apagada (já mergeada)", wt.branch));
        }
        let prefix = format!("refs/maestro/{}/", inv.id);
        for r in git(&wt.repo, &["for-each-ref", "--format=%(refname)", &prefix])
            .unwrap_or_default()
            .lines()
        {
            let _ = git(&wt.repo, &["update-ref", "-d", r]);
        }
        if let Some(parent) = wt.path.parent() {
            // `<repo>.wt` sai junto quando fica vazio
            let _ = std::fs::remove_dir(parent);
        }
    }
    let _ = std::fs::remove_file(data_dir().join(format!("{}.json", inv.id)));
    let _ = std::fs::remove_file(turns_file(&inv.id));
    let _ = std::fs::remove_file(preflight_file(&inv.id));
    for ext in ["merged", "sent", "hist", "context.md"] {
        let _ = std::fs::remove_file(data_dir().join(format!("{}.{ext}", inv.id)));
    }
    Ok(done)
}

/// Foto de todos os arquivos da worktree (inclusive os novos) num commit solto, sem tocar no índice nem na branch.
pub fn snapshot(root: &Path) -> Result<String, String> {
    let index = std::env::temp_dir().join(format!("maestro-idx-{}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let run = |args: &[&str]| -> Result<String, String> {
        let o = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            // a foto não precisa (nem deve) levar a identidade de quem usa; sem isto falha em máquina sem user.name
            .envs([
                ("GIT_AUTHOR_NAME", "Maestro"),
                ("GIT_AUTHOR_EMAIL", "maestro@localhost"),
                ("GIT_COMMITTER_NAME", "Maestro"),
                ("GIT_COMMITTER_EMAIL", "maestro@localhost"),
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
    };
    run(&["read-tree", "HEAD"])?;
    run(&["add", "-A"])?;
    let tree = run(&["write-tree"])?;
    let sha = run(&[
        "commit-tree",
        &tree,
        "-p",
        "HEAD",
        "-m",
        "maestro checkpoint",
    ])?;
    let _ = std::fs::remove_file(&index);
    Ok(sha)
}

/// Volta a worktree para uma foto, byte a byte: apaga o que surgiu depois e restaura o resto. O índice fica.
pub fn restore(root: &Path, sha: &str) -> Result<(), String> {
    let now = snapshot(root)?;
    for f in git(root, &["diff", "--name-only", "--diff-filter=A", sha, &now])
        .unwrap_or_default()
        .lines()
    {
        let _ = std::fs::remove_file(root.join(f));
    }
    git_err(
        root,
        &[
            "restore",
            &format!("--source={sha}"),
            "--worktree",
            "--",
            ".",
        ],
    )
}

/// Um turno do Claude: a foto de antes do prompt N.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub n: usize,
    pub sha: String,
    pub prompt: String,
}

fn turns_file(id: &str) -> PathBuf {
    data_dir().join(format!("{id}.turns"))
}

/// Uma lista que sobrevive ao reinício (`<id>.merged`: MRs já oferecidos; `<id>.sent`: comentários já enviados).
pub fn state_lines(id: &str, ext: &str) -> Vec<String> {
    std::fs::read_to_string(data_dir().join(format!("{id}.{ext}")))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

pub fn add_state_line(id: &str, ext: &str, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join(format!("{id}.{ext}")))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// A árvore dos arquivos que passaram no pré-voo (ou tiveram override).
pub fn preflight_file(id: &str) -> PathBuf {
    data_dir().join(format!("{id}.preflight"))
}

pub fn turns(id: &str) -> Vec<Turn> {
    std::fs::read_to_string(turns_file(id))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            Some(Turn {
                n: it.next()?.parse().ok()?,
                sha: it.next()?.into(),
                prompt: it.next().unwrap_or("").into(),
            })
        })
        .collect()
}

/// O hook `UserPromptSubmit` da worktree chama isto (`maestro checkpoint <id>`) com o JSON do Claude no stdin.
/// O hook UserPromptSubmit: grava o turno e, num `/mr` sem pré-voo, devolve o motivo do bloqueio.
pub fn checkpoint(id: &str, stdin: &str) -> Result<Option<String>, String> {
    let inv = inventories()
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("feature desconhecida")?;
    let root = &inv.worktrees[0].path;
    let sha = snapshot(root)?;
    let prompt = serde_json::from_str::<serde_json::Value>(stdin)
        .ok()
        .and_then(|v| v["prompt"].as_str().map(|p| p.replace(['\n', '\t'], " ")))
        .unwrap_or_default();
    // bloqueado não vira turno: o Claude nem recebe o prompt
    if let Some(reason) = crate::preflight::gate(id, root, &sha, &prompt) {
        return Ok(Some(reason));
    }
    let n = turns(id).len() + 1;
    git_err(
        root,
        &["update-ref", &format!("refs/maestro/{id}/t{n}"), &sha],
    )?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(turns_file(id))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{n}\t{sha}\t{prompt}").map_err(|e| e.to_string())?;
    Ok(None)
}

/// Hook só nesta worktree (`.claude/settings.local.json`, fora do git pelo `info/exclude`): o settings global fica intacto.
pub fn install_hook(repo: &Path, worktree: &Path, id: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = worktree.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = sh_quote(&exe.to_string_lossy());
    let hook = |cmd: String| json!([ { "hooks": [ { "type": "command", "command": cmd } ] } ]);
    let settings = json!({
        "hooks": {
            "UserPromptSubmit": hook(format!("{exe} checkpoint {id}")),
            "PermissionRequest": hook(format!("{exe} perm {id}")),
        },
        "permissions": { "allow": crate::perms::personal(repo) },
    });
    std::fs::write(
        dir.join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let exclude = repo.join(".git/info/exclude");
    let text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.lines().any(|l| l == ".claude/settings.local.json") {
        let _ = std::fs::create_dir_all(exclude.parent().unwrap());
        std::fs::write(
            &exclude,
            format!(
                "{text}{}.claude/settings.local.json\n",
                if text.ends_with('\n') || text.is_empty() {
                    ""
                } else {
                    "\n"
                }
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Tentativa descartada no melhor de N: conclui e apaga a branch mesmo sem merge (foi você que escolheu).
pub fn discard(herdr: &Herdr, inv: &Inventory) -> Result<(), String> {
    conclude(herdr, inv)?;
    for w in inv.worktrees.iter().filter(|w| w.created_branch) {
        if branches(&w.repo).contains(&w.branch) {
            git_err(&w.repo, &["branch", "-D", &w.branch])?;
        }
    }
    Ok(())
}

/// Ficou com esta: o grupo some do inventário e ela vira uma feature comum.
pub fn keep(inv: &Inventory) -> Result<(), String> {
    let mut i = inv.clone();
    i.group = None;
    save(&i).map_err(|e| e.to_string())
}

/// Repositórios irmãos do da feature (`~/repos-ctbz/*` com `.git`), para o "+ repo".
pub fn sibling_repos(repo: &Path) -> Vec<PathBuf> {
    let Some(parent) = repo.parent() else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p != repo && p.join(".git").is_dir())
        .collect();
    out.sort();
    out
}

/// "+ repo": worktree na mesma branch em outro repositório, só quando pedido, registrada no inventário.
pub fn add_repo(herdr: &Herdr, inv: &mut Inventory, repo: &Path) -> Result<PathBuf, String> {
    if inv.worktrees.iter().any(|w| w.repo == repo) {
        return Err(format!("{} já faz parte da feature", repo.display()));
    }
    let path = worktree_path(repo, &inv.branch);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let exists = branches(repo).contains(&inv.branch);
    let base = bases(repo)
        .into_iter()
        .next()
        .ok_or("o repositório não tem branch base")?;
    let p = path.to_string_lossy().into_owned();
    if exists {
        git_err(repo, &["worktree", "add", &p, &inv.branch])?;
    } else {
        git_err(repo, &["worktree", "add", "-b", &inv.branch, &p, &base])?;
    }
    inv.worktrees.push(Worktree {
        repo: repo.to_path_buf(),
        path: path.clone(),
        branch: inv.branch.clone(),
        created_branch: !exists,
    });
    save(inv).map_err(|e| e.to_string())?;
    // o Claude da feature passa a enxergar a pasta nova
    let agents = herdr.agents().unwrap_or_default();
    let ws = inv.workspace_id.as_deref();
    if let Some(a) = agents.iter().find(|a| Some(a.workspace_id.as_str()) == ws) {
        let _ = herdr.call(
            "pane.send_text",
            json!({ "pane_id": a.pane_id, "text": format!("/add-dir {p}\r") }),
        );
    }
    Ok(path)
}

/// Algo no disco que nenhuma feature reconhece.
#[derive(Debug, Clone, PartialEq)]
pub enum Orphan {
    /// worktree em `<repo>.wt/` (onde o Maestro cria as dele) sem inventário
    Worktree {
        repo: PathBuf,
        path: PathBuf,
        dirty: bool,
    },
    /// inventário cuja worktree já não existe
    Inventory { id: String, branch: String },
    /// `refs/maestro/<id>/…` de uma feature que não existe mais
    Ref { repo: PathBuf, name: String },
    /// projeto do compose `<repo>-…` parado e sem feature
    Compose { name: String },
    /// branch local já mergeada na base, sem worktree; pode ser sua, por isso vem desmarcada
    Branch { repo: PathBuf, name: String },
}

impl Orphan {
    pub fn label(&self) -> (String, String) {
        match self {
            Orphan::Worktree { path, dirty, .. } => (
                path.display().to_string(),
                if *dirty {
                    "worktree sem feature, com mudanças não commitadas".into()
                } else {
                    "worktree sem feature".into()
                },
            ),
            Orphan::Inventory { branch, .. } => (
                branch.clone(),
                "inventário de feature cuja worktree sumiu".into(),
            ),
            Orphan::Ref { name, .. } => (name.clone(), "ref de feature removida".into()),
            Orphan::Compose { name } => (name.clone(), "compose parado".into()),
            Orphan::Branch { name, .. } => (name.clone(), "branch local mergeada".into()),
        }
    }

    /// Marcado por padrão: só o que é seguro apagar sem olhar.
    pub fn safe(&self) -> bool {
        !matches!(
            self,
            Orphan::Branch { .. } | Orphan::Worktree { dirty: true, .. }
        )
    }

    pub fn remove(&self) -> Result<(), String> {
        match self {
            Orphan::Worktree { repo, path, .. } => {
                git_err(
                    repo,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                )?;
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
                Ok(())
            }
            Orphan::Inventory { id, .. } => {
                std::fs::remove_file(data_dir().join(format!("{id}.json")))
                    .map_err(|e| e.to_string())
            }
            Orphan::Ref { repo, name } => git_err(repo, &["update-ref", "-d", name]),
            Orphan::Compose { name } => Command::new("docker")
                .args(["compose", "-p", name, "down"])
                .output()
                .map_err(|e| e.to_string())
                .map(drop),
            Orphan::Branch { repo, name } => git_err(repo, &["branch", "-d", name]),
        }
    }
}

/// Compara o disco com os inventários nos repositórios dados (os das features e os abertos no herdr).
pub fn orphans(repos: &[PathBuf]) -> Vec<Orphan> {
    let invs = inventories();
    let known_paths: Vec<&Path> = invs
        .iter()
        .flat_map(|i| i.worktrees.iter().map(|w| w.path.as_path()))
        .collect();
    let mut out = Vec::new();
    for inv in &invs {
        if inv.worktrees.iter().all(|w| !w.path.exists()) {
            out.push(Orphan::Inventory {
                id: inv.id.clone(),
                branch: inv.branch.clone(),
            });
        }
    }
    let mut repos: Vec<PathBuf> = repos
        .iter()
        .cloned()
        .chain(invs.iter().map(|i| i.repo.clone()))
        .collect();
    repos.sort();
    repos.dedup();
    for repo in &repos {
        let wt_dir = worktree_path(repo, "x")
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let list = git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
        let mut wt_branches = Vec::new();
        for block in list.split("\n\n") {
            let path = block
                .lines()
                .find_map(|l| l.strip_prefix("worktree "))
                .map(PathBuf::from);
            if let Some(b) = block
                .lines()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
            {
                wt_branches.push(b.to_string());
            }
            if let Some(path) =
                path.filter(|p| p.starts_with(&wt_dir) && !known_paths.contains(&p.as_path()))
            {
                let dirty =
                    git(&path, &["status", "--porcelain"]).is_some_and(|s| !s.trim().is_empty());
                out.push(Orphan::Worktree {
                    repo: repo.clone(),
                    path,
                    dirty,
                });
            }
        }
        for r in git(
            repo,
            &["for-each-ref", "--format=%(refname)", "refs/maestro/"],
        )
        .unwrap_or_default()
        .lines()
        {
            let id = r
                .trim_start_matches("refs/maestro/")
                .split('/')
                .next()
                .unwrap_or_default();
            if !invs.iter().any(|i| i.id == id) {
                out.push(Orphan::Ref {
                    repo: repo.clone(),
                    name: r.into(),
                });
            }
        }
        let base = bases(repo).into_iter().next().unwrap_or_default();
        let current = git(repo, &["branch", "--show-current"]).unwrap_or_default();
        for b in git(
            repo,
            &["branch", "--merged", &base, "--format=%(refname:short)"],
        )
        .unwrap_or_default()
        .lines()
        {
            let in_use = b == base || b == current.trim() || wt_branches.iter().any(|w| w == b);
            if !in_use
                && !invs
                    .iter()
                    .any(|i| i.worktrees.iter().any(|w| w.branch == b))
            {
                out.push(Orphan::Branch {
                    repo: repo.clone(),
                    name: b.into(),
                });
            }
        }
        // só compose cujo arquivo está numa worktree do Maestro; prefixo de nome pegaria projeto de outro repo
        for (name, file) in stopped_compose_projects() {
            if file.starts_with(&wt_dir) && !invs.iter().any(|i| i.compose_project == name) {
                out.push(Orphan::Compose { name });
            }
        }
    }
    out
}

/// (nome, primeiro arquivo do compose) dos projetos parados.
fn stopped_compose_projects() -> Vec<(String, PathBuf)> {
    let Ok(o) = Command::new("docker")
        .args(["compose", "ls", "-a", "--format", "json"])
        .output()
    else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["Status"].as_str().is_some_and(|s| !s.contains("running")))
        .filter_map(|p| {
            let file = p["ConfigFiles"].as_str()?.split(',').next()?;
            Some((p["Name"].as_str()?.to_string(), PathBuf::from(file)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(prompt: &str) -> Request {
        Request {
            repo: "/r/bots-api".into(),
            branch: "feat/x".into(),
            base: "main".into(),
            prompt: prompt.into(),
            copy_env: true,
            setup: true,
            claude: true,
            model: None,
            group: None,
        }
    }

    #[test]
    fn nomes_portas_e_comando() {
        assert_eq!(slug("feat/PRD-1 Paginação!"), "feat-prd-1-paginacao");
        assert_eq!(
            worktree_path(Path::new("/r/bots-api"), "feat/health"),
            PathBuf::from("/r/bots-api.wt/feat-health")
        );
        assert_eq!(
            compose_project(Path::new("/r/bots-api"), "feat/health"),
            "bots-api-feat-health"
        );
        assert_eq!(next_port(&[3001, 3003]), 3002);
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(
            start_command(&req("faz X"), Some("npm ci"), None),
            "npm ci && claude 'faz X'"
        );
        let mut r = req("");
        r.setup = false;
        r.model = Some("sonnet");
        assert_eq!(
            start_command(&r, Some("npm ci"), None),
            "claude --model sonnet"
        );
        assert_eq!(
            start_command(&r, None, Some(Path::new("/d/x.context.md"))),
            "claude --model sonnet --append-system-prompt \"$(cat '/d/x.context.md')\""
        );
    }

    #[test]
    fn setup_e_env_de_um_repo() {
        let dir = std::env::temp_dir().join(format!("maestro-feat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join(".gitignore"), ".env\n.env.local\n").unwrap();
        std::fs::write(dir.join(".env"), "A=1").unwrap();
        std::fs::write(dir.join(".env.local"), "B=2").unwrap();
        std::fs::write(dir.join(".env.example"), "A=").unwrap();
        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        let mut env = env_files(&dir);
        env.sort();
        assert_eq!(env, vec![".env", ".env.local"]);
        assert_eq!(setup_command(&dir).as_deref(), Some("npm ci"));
        std::fs::write(dir.join(".maestro.toml"), "setup = \"make deps\"\n").unwrap();
        assert_eq!(setup_command(&dir).as_deref(), Some("make deps"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mais_um_repo_e_conclusao_voltam_ao_estado_de_antes() {
        let root = std::env::temp_dir().join(format!("maestro-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (app, k8s) = (root.join("app"), root.join("k8s-configmap"));
        for r in [&app, &k8s] {
            std::fs::create_dir_all(r).unwrap();
            for args in [
                &["init", "-q", "-b", "main"][..],
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "i",
                ],
            ] {
                Command::new("git")
                    .arg("-C")
                    .arg(r)
                    .args(args)
                    .output()
                    .unwrap();
            }
        }
        let state = |r: &Path| {
            (
                git(r, &["worktree", "list", "--porcelain"])
                    .unwrap()
                    .lines()
                    .filter(|l| l.starts_with("worktree"))
                    .count(),
                git(r, &["branch", "--format=%(refname:short)"]).unwrap(),
            )
        };
        let before = (state(&app), state(&k8s));
        let h = Herdr::offline();
        let wt = worktree_path(&app, "feat/multi");
        git_err(
            &app,
            &[
                "worktree",
                "add",
                "-b",
                "feat/multi",
                &wt.to_string_lossy(),
                "main",
            ],
        )
        .unwrap();
        let mut inv = Inventory {
            id: format!("teste-multi-{}", std::process::id()),
            repo: app.clone(),
            branch: "feat/multi".into(),
            base: "main".into(),
            worktrees: vec![Worktree {
                repo: app.clone(),
                path: wt,
                branch: "feat/multi".into(),
                created_branch: true,
            }],
            port: 3999,
            compose_project: "app-feat-multi-teste".into(),
            workspace_id: None,
            group: None,
            model: None,
            created_at: 0,
        };
        assert_eq!(sibling_repos(&app), vec![k8s.clone()]);
        let extra = add_repo(&h, &mut inv, &k8s).unwrap();
        assert!(extra.ends_with("k8s-configmap.wt/feat-multi") && extra.exists());
        assert!(add_repo(&h, &mut inv, &k8s).is_err());
        assert_eq!(pending(&inv).uncommitted, 0);
        conclude(&h, &inv).unwrap();
        assert_eq!((state(&app), state(&k8s)), before);
        assert!(!root.join("app.wt").exists() && !root.join("k8s-configmap.wt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foto_e_volta_byte_a_byte() {
        let dir = std::env::temp_dir().join(format!("maestro-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
        ]);
        std::fs::write(dir.join("a.txt"), "2 sem newline").unwrap();
        std::fs::write(dir.join("novo.txt"), "n\n").unwrap();
        let status =
            || String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).into_owned();
        let before = status();
        let sha = snapshot(&dir).unwrap();
        assert_eq!(status(), before, "a foto não mexe no índice");
        std::fs::write(dir.join("a.txt"), "3\n").unwrap();
        std::fs::remove_file(dir.join("b.txt")).unwrap();
        std::fs::write(dir.join("depois.txt"), "d\n").unwrap();
        std::fs::remove_file(dir.join("novo.txt")).unwrap();
        restore(&dir, &sha).unwrap();
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"2 sem newline");
        assert_eq!(std::fs::read(dir.join("b.txt")).unwrap(), b"b\n");
        assert_eq!(std::fs::read(dir.join("novo.txt")).unwrap(), b"n\n");
        assert!(!dir.join("depois.txt").exists());
        assert_eq!(status(), before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn acha_orfaos_sem_tocar_no_resto() {
        let root = std::env::temp_dir().join(format!("maestro-orf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        run(&repo, &["init", "-q", "-b", "main"]);
        run(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        );
        run(&repo, &["branch", "velha"]);
        let wt = worktree_path(&repo, "feat/sobra");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/sobra",
                &wt.to_string_lossy(),
                "main",
            ],
        );
        run(&repo, &["update-ref", "refs/maestro/sumiu-1/t1", "HEAD"]);
        let fora = root.join("minha-worktree");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "minha",
                &fora.to_string_lossy(),
                "main",
            ],
        );
        let o = orphans(std::slice::from_ref(&repo));
        assert!(o.contains(&Orphan::Worktree {
            repo: repo.clone(),
            path: wt.clone(),
            dirty: false
        }));
        assert!(o.contains(&Orphan::Ref {
            repo: repo.clone(),
            name: "refs/maestro/sumiu-1/t1".into()
        }));
        assert!(o.contains(&Orphan::Branch {
            repo: repo.clone(),
            name: "velha".into()
        }));
        // worktree fora de `<repo>.wt/` não é do Maestro; branch em uso por worktree também não sai
        assert!(
            !o.iter()
                .any(|x| matches!(x, Orphan::Worktree { path, .. } if *path == fora))
        );
        assert!(!o.iter().any(
            |x| matches!(x, Orphan::Branch { name, .. } if name == "minha" || name == "feat/sobra")
        ));
        assert!(
            !Orphan::Branch {
                repo: repo.clone(),
                name: "velha".into()
            }
            .safe()
        );
        // só o que é deste repo de teste: inventários e composes reais da máquina não entram
        let mine = |x: &Orphan| matches!(x, Orphan::Worktree { repo: r, .. } | Orphan::Ref { repo: r, .. } if *r == repo);
        for x in o.iter().filter(|x| x.safe() && mine(x)) {
            x.remove().unwrap();
        }
        assert!(
            orphans(std::slice::from_ref(&repo))
                .iter()
                .filter(|x| mine(x))
                .all(|x| !x.safe())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
