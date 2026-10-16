//! Cliente mínimo de language server: um processo por (worktree, linguagem), JSON-RPC no stdio.

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
};

use futures::channel::oneshot;
use serde_json::{Value, json};

/// Linguagem, binários aceitos (o primeiro achado vence), argumentos e o comando de instalação.
pub struct Server {
    pub lang: &'static str,
    pub id: &'static str,
    /// (binário, argumentos), na ordem de preferência
    bins: &'static [(&'static str, &'static [&'static str])],
    pub install: &'static str,
}

const SERVERS: &[(&[&str], Server)] = &[
    (
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        Server {
            lang: "TypeScript",
            id: "typescript",
            // o primeiro segue o TypeScript do projeto; sem node_modules, o do TS 7 global
            bins: &[
                ("typescript-language-server", &["--stdio"]),
                ("tsc", &["--lsp", "--stdio"]),
            ],
            install: "npm i -g typescript-language-server typescript",
        },
    ),
    (
        &["rs"],
        Server {
            lang: "Rust",
            id: "rust",
            bins: &[("rust-analyzer", &[])],
            install: "rustup component add rust-analyzer",
        },
    ),
    (
        &["go"],
        Server {
            lang: "Go",
            id: "go",
            bins: &[("gopls", &[])],
            install: "go install golang.org/x/tools/gopls@latest",
        },
    ),
    (
        &["py"],
        Server {
            lang: "Python",
            id: "python",
            bins: &[
                ("basedpyright-langserver", &["--stdio"]),
                ("pyright-langserver", &["--stdio"]),
            ],
            install: "npm i -g pyright",
        },
    ),
    (
        &["json"],
        Server {
            lang: "JSON",
            id: "json",
            bins: &[("vscode-json-language-server", &["--stdio"])],
            install: "npm i -g vscode-langservers-extracted",
        },
    ),
];

pub fn server_for(path: &str) -> Option<&'static Server> {
    let ext = Path::new(path).extension()?.to_str()?;
    SERVERS
        .iter()
        .find(|(exts, _)| exts.contains(&ext))
        .map(|(_, s)| s)
}

/// `languageId` do `didOpen`.
pub fn language_id(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("jsx") => "javascriptreact",
        Some("rs") => "rust",
        Some("go") => "go",
        Some("py") => "python",
        Some("json") => "json",
        _ => "plaintext",
    }
}

/// O app aberto pelo dock não tem o PATH do zsh: procura também no nvm, no mason e no cargo.
fn find(bin: &str) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push(home.join(".local/share/nvim/mason/bin"));
    dirs.push(home.join(".cargo/bin"));
    dirs.push(home.join(".local/go/packages/bin"));
    dirs.push(home.join("go/bin"));
    if let Ok(nvm) = std::fs::read_dir(home.join(".config/nvm/versions/node")) {
        let mut v: Vec<PathBuf> = nvm.flatten().map(|e| e.path().join("bin")).collect();
        v.sort();
        dirs.extend(v.into_iter().rev());
    }
    dirs.into_iter().map(|d| d.join(bin)).find(|p| p.is_file())
}

pub fn file_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' | b'@' => {
                s.push(b as char)
            }
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(raw[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

pub fn frame(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

pub fn read_message(r: &mut impl BufRead) -> io::Result<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let len = len.ok_or_else(|| io::Error::other("sem Content-Length"))?;
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next: AtomicI64,
    pending: Pending,
    diagnostics: Arc<Mutex<HashMap<String, Vec<lsp_types::Diagnostic>>>>,
    /// sobe a cada `publishDiagnostics`: o editor compara com o último que aplicou
    pub generation: Arc<AtomicU64>,
    pub server: &'static Server,
    /// o binário que subiu, para a barra de status
    pub bin: String,
    /// o servidor só entrega diagnóstico quando pedido (`textDocument/diagnostic`), como o TS 7
    pull: AtomicBool,
}

impl Client {
    fn spawn(root: &Path, server: &'static Server) -> Result<Arc<Self>, String> {
        let (bin, args) = server
            .bins
            .iter()
            .filter(|(b, _)| usable(root, b))
            .find_map(|(b, a)| Some((find(b)?, *a)))
            .ok_or_else(|| format!("{} não encontrado", server.bins[0].0))?;
        let mut cmd = Command::new(&bin);
        // o node do nvm precisa estar no PATH do servidor de TypeScript
        if let Some(dir) = bin.parent() {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut dirs = vec![dir.to_path_buf()];
            dirs.extend(std::env::split_paths(&path));
            cmd.env("PATH", std::env::join_paths(dirs).unwrap_or(path));
        }
        let mut child = cmd
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", bin.display()))?;
        let stdin = child.stdin.take().ok_or("sem stdin")?;
        let stdout = child.stdout.take().ok_or("sem stdout")?;
        let client = Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next: AtomicI64::new(1),
            pending: Arc::default(),
            diagnostics: Arc::default(),
            generation: Arc::default(),
            server,
            bin: bin
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            pull: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&client);
        let (pending, diagnostics, generation) = (
            client.pending.clone(),
            client.diagnostics.clone(),
            client.generation.clone(),
        );
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Ok(msg) = read_message(&mut r) {
                let id = msg["id"].as_i64();
                match (id, msg["method"].as_str()) {
                    (Some(id), None) => {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let res = match msg.get("error") {
                                Some(e) => Err(e["message"].as_str().unwrap_or("erro").into()),
                                None => Ok(msg["result"].clone()),
                            };
                            let _ = tx.send(res);
                        }
                    }
                    (_, Some("textDocument/publishDiagnostics")) => {
                        let p = &msg["params"];
                        let list =
                            serde_json::from_value(p["diagnostics"].clone()).unwrap_or_default();
                        let uri = p["uri"].as_str().unwrap_or_default().to_string();
                        diagnostics.lock().unwrap().insert(uri, list);
                        generation.fetch_add(1, Ordering::Relaxed);
                    }
                    (_, Some(method)) if !msg["id"].is_null() => {
                        // pedidos do servidor: responde o mínimo para ele não travar esperando
                        let result = match method {
                            "workspace/configuration" => {
                                json!(vec![
                                    Value::Null;
                                    msg["params"]["items"].as_array().map_or(0, Vec::len)
                                ])
                            }
                            _ => Value::Null,
                        };
                        if let Some(c) = weak.upgrade() {
                            c.send(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}));
                        }
                    }
                    _ => {}
                }
            }
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(Err("o servidor de linguagem fechou".into()));
            }
        });
        Ok(client)
    }

    fn send(&self, msg: &Value) {
        let mut w = self.stdin.lock().unwrap();
        let _ = w.write_all(&frame(msg)).and_then(|_| w.flush());
    }

    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        rx
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn diagnostics(&self, uri: &str) -> Vec<lsp_types::Diagnostic> {
        self.diagnostics
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    fn initialize(&self, root: &Path) -> Result<(), String> {
        let uri = file_uri(root);
        let caps = json!({
            "textDocument": {
                "synchronization": {"didSave": true},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "definition": {"linkSupport": true},
                "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["markdown", "plaintext"]}},
                "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "source"]}}, "resolveSupport": {"properties": ["edit"]}, "dataSupport": true},
                "publishDiagnostics": {"relatedInformation": false},
                "diagnostic": {"dynamicRegistration": false}
            },
            "workspace": {"configuration": true, "workspaceFolders": true},
            "window": {"workDoneProgress": false}
        });
        let rx = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()}],
                "capabilities": caps,
            }),
        );
        let init =
            futures::executor::block_on(rx).map_err(|_| "o servidor fechou".to_string())??;
        self.pull.store(
            !init["capabilities"]["diagnosticProvider"].is_null(),
            Ordering::Relaxed,
        );
        self.notify("initialized", json!({}));
        Ok(())
    }

    pub fn did_open(self: &Arc<Self>, path: &Path, text: &str) {
        let uri = file_uri(path);
        let lang = language_id(&path.to_string_lossy());
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": lang, "version": 1, "text": text}}),
        );
        self.pull_diagnostics(uri);
    }

    // ponytail: sync completo a cada mudança; incremental se arquivo grande pesar
    pub fn did_change(self: &Arc<Self>, path: &Path, version: i32, text: &str) {
        let uri = file_uri(path);
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
        self.pull_diagnostics(uri);
    }

    pub fn did_save(&self, path: &Path) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": file_uri(path)}}),
        );
    }

    pub fn did_close(&self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": file_uri(path)}}),
        );
    }

    fn pull_diagnostics(self: &Arc<Self>, uri: String) {
        if !self.pull.load(Ordering::Relaxed) {
            return;
        }
        let rx = self.request(
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": uri}}),
        );
        let (diagnostics, generation) = (self.diagnostics.clone(), self.generation.clone());
        std::thread::spawn(move || {
            let Ok(Ok(r)) = futures::executor::block_on(rx) else {
                return;
            };
            if r["kind"] != "full" {
                return;
            }
            let list = serde_json::from_value(r["items"].clone()).unwrap_or_default();
            diagnostics.lock().unwrap().insert(uri, list);
            generation.fetch_add(1, Ordering::Relaxed);
        });
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

type Key = (PathBuf, &'static str);

fn registry() -> &'static Mutex<HashMap<Key, Arc<Client>>> {
    static R: std::sync::OnceLock<Mutex<HashMap<Key, Arc<Client>>>> = std::sync::OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// O servidor desta worktree e linguagem, iniciado na primeira vez. Bloqueia no `initialize`: chamar fora da thread de UI.
pub fn client(root: &Path, server: &'static Server) -> Result<Arc<Client>, String> {
    let key = (root.to_path_buf(), server.id);
    if let Some(c) = registry().lock().unwrap().get(&key) {
        return Ok(c.clone());
    }
    let c = Client::spawn(root, server)?;
    c.initialize(root)?;
    registry().lock().unwrap().insert(key, c.clone());
    Ok(c)
}

/// Ao remover a feature: derruba os servidores daquela worktree.
pub fn stop(root: &Path) {
    registry().lock().unwrap().retain(|(r, _), _| r != root);
}

/// O typescript-language-server só funciona com o tsserver do projeto (o TS 7 não tem mais).
fn usable(root: &Path, bin: &str) -> bool {
    bin != "typescript-language-server"
        || root
            .join("node_modules/typescript/lib/tsserver.js")
            .is_file()
}

pub fn installed(root: &Path, server: &Server) -> bool {
    server
        .bins
        .iter()
        .any(|(b, _)| usable(root, b) && find(b).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enquadramento() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": "ç"}});
        let mut two = frame(&msg);
        two.extend(frame(&json!({"method": "x"})));
        let mut r = io::Cursor::new(two);
        assert_eq!(read_message(&mut r).unwrap(), msg);
        assert_eq!(read_message(&mut r).unwrap()["method"], "x");
        assert!(read_message(&mut r).is_err());
    }

    /// Precisa do typescript-language-server: `cargo test -- --ignored lsp`
    #[test]
    #[ignore]
    fn typescript_de_verdade() {
        let dir = std::env::temp_dir().join(format!("maestro-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        let text = "export function soma(x: number, y: number) {\n  return x + y\n}\nconst n: string = soma(1, 2)\nn.\n";
        std::fs::write(&file, text).unwrap();
        let c = client(&dir, server_for("a.ts").unwrap()).unwrap();
        let uri = file_uri(&file);
        c.did_open(&file, text);
        let t0 = std::time::Instant::now();
        while c.diagnostics(&uri).is_empty() && t0.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let d = c.diagnostics(&uri);
        assert!(d.iter().any(|d| d.range.start.line == 3), "{d:?}");
        let pos = |line, character| json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}});
        let hover = futures::executor::block_on(c.request("textDocument/hover", pos(0, 17)))
            .unwrap()
            .unwrap();
        assert!(hover.to_string().contains("soma"), "{hover}");
        let comp = futures::executor::block_on(c.request("textDocument/completion", pos(4, 2)))
            .unwrap()
            .unwrap();
        assert!(comp.to_string().contains("toUpperCase"));
        stop(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn diagnostico_real(name: &str, files: &[(&str, &str)], line: u32) {
        let dir = std::env::temp_dir().join(format!("maestro-lsp-{name}-{}", std::process::id()));
        for (f, t) in files {
            std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
            std::fs::write(dir.join(f), t).unwrap();
        }
        let (f, t) = files[files.len() - 1];
        let c = client(&dir, server_for(f).unwrap()).unwrap();
        c.did_open(&dir.join(f), t);
        let uri = file_uri(&dir.join(f));
        let t0 = std::time::Instant::now();
        while c.diagnostics(&uri).is_empty() && t0.elapsed().as_secs() < 90 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let d = c.diagnostics(&uri);
        stop(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(d.iter().any(|d| d.range.start.line == line), "{d:?}");
    }

    #[test]
    #[ignore]
    fn go_de_verdade() {
        diagnostico_real(
            "go",
            &[
                ("go.mod", "module x\n\ngo 1.21\n"),
                (
                    "main.go",
                    "package main\n\nfunc main() {\n\tvar n string = 1\n}\n",
                ),
            ],
            3,
        );
    }

    #[test]
    #[ignore]
    fn rust_de_verdade() {
        diagnostico_real(
            "rs",
            &[
                (
                    "Cargo.toml",
                    "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                ("src/main.rs", "fn main() {\n    let n: String = 1;\n}\n"),
            ],
            1,
        );
    }

    #[test]
    fn uri_de_arquivo() {
        let p = Path::new("/home/a.b@X.COM/repo com espaço/a.ts");
        let u = file_uri(p);
        assert_eq!(u, "file:///home/a.b@X.COM/repo%20com%20espa%C3%A7o/a.ts");
        assert_eq!(uri_path(&u).unwrap(), p);
        assert_eq!(server_for("src/a.tsx").unwrap().id, "typescript");
        assert!(server_for("README.md").is_none());
    }
}
