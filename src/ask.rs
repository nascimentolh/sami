//! "Perguntar ao Claude" sobre um trecho do diff (só leitura) e o comando por descrição (Ctrl+Shift+K).

use std::{
    io::{BufRead, BufReader, Write},
    ops::Range,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use futures::StreamExt;
use gpui_kit::{prelude::FluentBuilder, *};
use serde_json::Value;

use crate::{
    diff::{FileDiff, Kind},
    form::{FieldEvent, TextField, button},
    herdr::Herdr,
    theme::{self, Theme},
    vault_view::{Span, inline},
};

/// O trecho perguntado: de onde o Claude lê e o que ele recebe no prompt.
#[derive(Clone, Debug)]
pub struct Ctx {
    /// worktree da feature; no MR, `None` até `mrs::worktree` preparar a dele
    pub root: Option<PathBuf>,
    /// (path do projeto, iid, branch de destino) do MR
    pub mr: Option<(String, u64, String)>,
    /// `src/handler.ts:10-17`
    pub range: String,
    pub patch: String,
    /// "worktree do MR experts-nfs !276" / "worktree de feat/x"
    pub place: String,
}

impl Ctx {
    pub fn of_hunk(f: &FileDiff, hi: usize, place: String) -> Option<Self> {
        let h = f.hunks.get(hi)?;
        let nums: Vec<u32> = h
            .lines
            .iter()
            .filter_map(|l| match l.kind {
                Kind::Del => l.old,
                _ => l.new,
            })
            .collect();
        let (a, b) = (nums.iter().min()?, nums.iter().max()?);
        Some(Self {
            root: None,
            mr: None,
            range: format!("{}:{a}-{b}", f.path),
            patch: f.hunk_patch(hi),
            place,
        })
    }
}

/// O bloco do arquivo que contém a linha (nova ou, se só existir do lado velho, velha).
pub fn hunk_at(f: &FileDiff, line: u32) -> usize {
    f.hunks
        .iter()
        .position(|h| h.lines.iter().any(|l| l.new.or(l.old) == Some(line)))
        .unwrap_or(0)
}

#[derive(Debug, PartialEq)]
pub enum Ev {
    Pid(u32),
    Tool(String),
    Text(String),
    Done { session: Option<String>, cost: f64 },
    Failed(String),
}

pub fn parse(line: &str) -> Option<Ev> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "stream_event" => {
            let d = &v["event"]["delta"];
            (v["event"]["type"] == "content_block_delta" && d["type"] == "text_delta")
                .then(|| Ev::Text(d["text"].as_str().unwrap_or_default().into()))
        }
        "assistant" => match crate::mrs::parse_event(line)? {
            crate::mrs::Step::Tool(t) => Some(Ev::Tool(t)),
            _ => None,
        },
        "result" if v["subtype"].as_str().is_some_and(|s| s.contains("budget")) => {
            Some(Ev::Failed("teto de gasto atingido".into()))
        }
        "result" if v["is_error"] == true || v["subtype"] != "success" => Some(Ev::Failed(
            v["result"].as_str().unwrap_or("o Claude falhou").into(),
        )),
        "result" => Some(Ev::Done {
            session: v["session_id"].as_str().map(String::from),
            cost: v["total_cost_usd"].as_f64().unwrap_or(0.),
        }),
        _ => None,
    }
}

/// As mesmas travas da auditoria: sem settings do usuário, sem MCP, só leitura.
pub fn args(resume: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--max-budget-usd",
        "0.50",
        "--restricted",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob,Bash",
        "--allowedTools",
        "Bash(git diff:*)",
        "Bash(git log:*)",
        "Bash(git blame:*)",
        "Bash(git show:*)",
        "Bash(git grep:*)",
        "Read",
        "Grep",
        "Glob",
    ]
    .map(String::from)
    .to_vec();
    if let Ok(m) = std::env::var("MAESTRO_ASK_MODEL") {
        a.extend(["--model".into(), m]);
    }
    if let Some(s) = resume {
        a.extend(["--resume".into(), s.into()]);
    }
    a
}

pub fn prompt(ctx: &Ctx, q: &str, first: bool) -> String {
    if !first {
        return q.into();
    }
    format!(
        "Você está na {place}, em modo SOMENTE LEITURA: não altere arquivos. \
         Responda em português, curto e direto, citando arquivo:linha quando ajudar.\n\n\
         O trecho em questão ({range}):\n```diff\n{patch}```\n\nPergunta: {q}",
        place = ctx.place,
        range = ctx.range,
        patch = ctx.patch,
    )
}

/// Roda uma pergunta; bloqueia até o `claude` terminar (chamar numa thread).
pub fn run(root: &Path, prompt: &str, resume: Option<&str>, send: impl Fn(Ev)) {
    let child = Command::new("claude")
        .args(args(resume))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return send(Ev::Failed(format!("claude: {e}"))),
    };
    send(Ev::Pid(child.id()));
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(prompt.as_bytes());
    }
    let mut done = false;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(ev) = parse(&line) {
                done |= matches!(ev, Ev::Done { .. } | Ev::Failed(_));
                send(ev);
            }
        }
    }
    let err = child
        .wait_with_output()
        .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
        .unwrap_or_default();
    if !done {
        send(Ev::Failed(if err.is_empty() {
            "o claude parou sem responder".into()
        } else {
            err
        }));
    }
}

struct Qa {
    q: String,
    tools: Vec<String>,
    a: String,
    err: Option<String>,
}

pub enum AskEvent {
    Close,
}

#[derive(Clone, Copy)]
struct AskDrag;

const SUGGESTIONS: [&str; 3] = [
    "Explica este bloco",
    "Isso quebra quem chama?",
    "Tem teste cobrindo?",
];

/// A gaveta "Perguntar ao Claude" do protótipo.
pub struct AskView {
    ctx: Ctx,
    qa: Vec<Qa>,
    busy: bool,
    cost: f64,
    session: Option<String>,
    pid: Option<u32>,
    input: Entity<TextField>,
    width: f32,
    scroll: ScrollHandle,
    theme: Theme,
}

impl EventEmitter<AskEvent> for AskView {}

impl AskView {
    pub fn new(ctx: Ctx, theme: Theme, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextField::new(
                theme.clone(),
                "",
                "Pergunte sobre este trecho…",
                false,
                false,
                cx,
            )
        });
        cx.subscribe(&input, |this, f, e: &FieldEvent, cx| match e {
            FieldEvent::Submit => {
                let q = f.read(cx).value.trim().to_string();
                if !q.is_empty() {
                    this.ask(q, cx);
                }
            }
            FieldEvent::Cancel => cx.emit(AskEvent::Close),
            FieldEvent::Changed => {}
        })
        .detach();
        Self {
            ctx,
            qa: Vec::new(),
            busy: false,
            cost: 0.,
            session: None,
            pid: None,
            input,
            width: 440.,
            scroll: ScrollHandle::new(),
            theme,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle()
    }

    fn ask(&mut self, q: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let first = self.qa.is_empty();
        let prompt = prompt(&self.ctx, &q, first);
        self.qa.push(Qa {
            q,
            tools: Vec::new(),
            a: String::new(),
            err: None,
        });
        self.busy = true;
        self.input.update(cx, |f, cx| {
            f.set(String::new(), cx);
            f.set_placeholder("Continuar a conversa…", cx);
        });
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        let (root, mr, resume) = (
            self.ctx.root.clone(),
            self.ctx.mr.clone(),
            self.session.clone(),
        );
        std::thread::spawn(move || {
            let root = match (root, mr) {
                (Some(r), _) => r,
                (None, Some((path, iid, target))) => {
                    // o Ask só sabe do MR aberto: a forja vem do repo em que o Maestro está
                    let f = std::env::current_dir()
                        .ok()
                        .and_then(|d| crate::forja::Forja::de(&d))
                        .unwrap_or(crate::forja::Forja::GitLab);
                    match crate::mrs::worktree(f, &path, iid, &target) {
                        Ok(r) => r,
                        Err(e) => return drop(tx.unbounded_send(Ev::Failed(e))),
                    }
                }
                (None, None) => return drop(tx.unbounded_send(Ev::Failed("sem worktree".into()))),
            };
            run(&root, &prompt, resume.as_deref(), |e| {
                drop(tx.unbounded_send(e))
            });
        });
        cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                let ok = this.update(cx, |this, cx| {
                    let Some(it) = this.qa.last_mut() else { return };
                    match ev {
                        Ev::Pid(p) => this.pid = Some(p),
                        Ev::Tool(t) => it.tools.push(t),
                        Ev::Text(t) => it.a.push_str(&t),
                        Ev::Done { session, cost } => {
                            this.session = session.or(this.session.take());
                            this.cost += cost;
                            this.busy = false;
                            this.pid = None;
                        }
                        Ev::Failed(e) => {
                            it.err = Some(e);
                            this.busy = false;
                            this.pid = None;
                        }
                    }
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn answer(&self, text: &str) -> StyledText {
        let t = &self.theme;
        let (text, spans) = inline(text);
        let hl: Vec<(Range<usize>, HighlightStyle)> = spans
            .iter()
            .map(|(r, s)| {
                let style = match s {
                    Span::Code => HighlightStyle {
                        background_color: Some(t.panel),
                        ..Default::default()
                    },
                    Span::Bold => HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                    Span::Link(_) => HighlightStyle::default(),
                };
                (r.clone(), style)
            })
            .collect();
        let mono: Vec<(Range<usize>, SharedString)> = spans
            .iter()
            .filter(|(_, s)| *s == Span::Code)
            .map(|(r, _)| (r.clone(), SharedString::from(t.mono.clone())))
            .collect();
        StyledText::new(text)
            .with_highlights(hl)
            .with_font_family_overrides(mono)
    }
}

impl Drop for AskView {
    fn drop(&mut self) {
        // fechar a gaveta para a pergunta em andamento: não paga por uma resposta que ninguém vai ler
        if let Some(p) = self.pid {
            let _ = Command::new("kill").arg(p.to_string()).status();
        }
    }
}

impl Render for AskView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div().flex().child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .border_1()
                        .border_color(t.line)
                        .rounded(px(4.))
                        .px(px(6.))
                        .py(px(2.))
                        .child(self.ctx.range.clone()),
                ),
            )
            .when(self.qa.is_empty(), |d| {
                d.child(div().text_color(t.muted).child(format!(
                    "O Claude lê o código da {} para responder. Ele não edita nada e a resposta não vira comentário.",
                    self.ctx.place
                )))
            });
        let last = self.qa.len().saturating_sub(1);
        for (i, x) in self.qa.iter().enumerate() {
            let mut qa = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_color(t.accent)
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("› {}", x.q)),
                )
                .children(x.tools.iter().map(|tl| {
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .text_color(t.comment)
                        .child(format!("↳ {tl}"))
                }));
            let cursor = if self.busy && i == last { "▍" } else { "" };
            qa = qa.child(
                div()
                    .line_height(relative(1.55))
                    .child(self.answer(&format!("{}{cursor}", x.a))),
            );
            if let Some(e) = &x.err {
                qa = qa.child(div().text_color(rgb(0xff5555)).child(e.clone()));
            }
            body = body.child(qa);
        }
        if !self.busy {
            body = body.child(div().flex().flex_wrap().gap(px(6.)).children(
                SUGGESTIONS.iter().enumerate().map(|(i, s)| {
                    button(&t, ("ask-sg", i), s, false).on_click(
                        cx.listener(move |this, _, _, cx| this.ask(SUGGESTIONS[i].into(), cx)),
                    )
                }),
            ));
        }
        let cost = if self.cost > 0. {
            format!(" · US$ {:.2}", self.cost).replace('.', ",")
        } else {
            String::new()
        };
        let resumed = if self.qa.len() > 1 {
            " · mesma sessão (--resume)"
        } else {
            ""
        };
        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(self.width))
            .bg(t.raise)
            .border_l_1()
            .border_color(t.line)
            .shadow_lg()
            .flex()
            .flex_col()
            .occlude()
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<AskDrag>, w, cx| {
                let right = e.bounds.right();
                let max = (f32::from(w.viewport_size().width) - 200.).max(320.);
                this.width = f32::from(right - e.event.position.x).clamp(320., max);
                cx.notify();
            }))
            .child(
                theme::resize_handle("rz-ask", AskDrag, &t)
                    .absolute()
                    .left_0()
                    .top_0(),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Perguntar ao Claude"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("ask-x")
                            .cursor_pointer()
                            .text_color(t.muted)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AskEvent::Close)))
                            .child("×"),
                    ),
            )
            .child(
                div()
                    .id("ask-db")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(14.))
                    .py(px(12.))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .child(div().flex_1().min_w_0().child(self.input.clone()))
                            .child(
                                button(&t, "ask-go", "Perguntar", true)
                                    .when(self.busy, |d| d.opacity(0.45))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let q = this.input.read(cx).value.trim().to_string();
                                        if !q.is_empty() {
                                            this.ask(q, cx);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.comment)
                            .child(format!(
                                "só leitura (Read, Grep, Glob, git) · nada é publicado{cost}{resumed}"
                            )),
                    ),
            )
    }
}

// ---------- Ctrl+Shift+K: comando por descrição ----------

actions!(kcmd, [NextOption]);

pub fn kcmd_args() -> Vec<String> {
    [
        "-p",
        "--model",
        "haiku",
        "--tools",
        "",
        "--restricted",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--output-format",
        "text",
    ]
    .map(String::from)
    .to_vec()
}

pub fn kcmd_prompt(what: &str, cwd: &str, branch: &str, os: &str, shell: &str) -> String {
    format!(
        "Escreva o comando de terminal para: {what}\n\n\
         Contexto: {os}, shell {shell}, cwd {cwd}{branch}.\n\
         Responda SÓ com até 3 alternativas, uma por linha, a melhor primeiro. \
         Sem explicação, sem markdown, sem numeração, sem `$`.",
        branch = if branch.is_empty() {
            String::new()
        } else {
            format!(", branch git {branch}")
        },
    )
}

/// As alternativas da resposta, limpas de cercas de código, `$ ` e numeração.
pub fn commands(out: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for l in out.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with("```") {
            continue;
        }
        let l = l
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')']);
        let l = l.strip_prefix("- ").unwrap_or(l).trim();
        let l = l.strip_prefix("$ ").unwrap_or(l).trim_matches('`').trim();
        if !l.is_empty() && !v.iter().any(|x| x == l) {
            v.push(l.into());
        }
    }
    v.truncate(3);
    v
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "Linux".into())
}

#[derive(PartialEq)]
enum KPhase {
    Ask,
    Think,
    Show,
    Fail(String),
}

pub enum KEvent {
    Close,
    Typed,
}

/// O `.kpop` do protótipo.
pub struct KcmdView {
    input: Entity<TextField>,
    phase: KPhase,
    opts: Vec<String>,
    i: usize,
    cwd: String,
    pane: Option<String>,
    herdr: Arc<Herdr>,
    theme: Theme,
}

impl EventEmitter<KEvent> for KcmdView {}

impl KcmdView {
    pub fn new(herdr: Arc<Herdr>, theme: Theme, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextField::new(
                theme.clone(),
                "",
                "Descreva o comando (ex.: pods com erro no namespace bots)",
                false,
                false,
                cx,
            )
        });
        cx.subscribe(&input, |this, _, e: &FieldEvent, cx| match e {
            FieldEvent::Submit => this.submit(cx),
            FieldEvent::Cancel => cx.emit(KEvent::Close),
            FieldEvent::Changed => {
                if this.phase != KPhase::Think {
                    this.phase = KPhase::Ask;
                    cx.notify();
                }
            }
        })
        .detach();
        let panes = herdr.panes().unwrap_or_default();
        let focused = panes["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["focused"] == true);
        let cwd = focused
            .and_then(|p| p["foreground_cwd"].as_str().or(p["cwd"].as_str()))
            .unwrap_or("~")
            .to_string();
        let pane = focused
            .and_then(|p| p["pane_id"].as_str())
            .map(String::from);
        Self {
            input,
            phase: KPhase::Ask,
            opts: Vec::new(),
            i: 0,
            cwd,
            pane,
            herdr,
            theme,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            KPhase::Show => {
                let (Some(pane), Some(cmd)) = (self.pane.clone(), self.opts.get(self.i).cloned())
                else {
                    return;
                };
                let herdr = self.herdr.clone();
                // sem "\r": o comando fica no prompt para você conferir e rodar
                std::thread::spawn(move || {
                    herdr.call(
                        "pane.send_text",
                        serde_json::json!({ "pane_id": pane, "text": cmd }),
                    )
                });
                cx.emit(KEvent::Typed);
            }
            KPhase::Think => {}
            KPhase::Ask | KPhase::Fail(_) => {
                let what = self.input.read(cx).value.trim().to_string();
                if what.is_empty() {
                    return;
                }
                self.phase = KPhase::Think;
                let cwd = self.cwd.clone();
                let task = cx.background_executor().spawn(async move {
                    let branch = crate::herdr::branch(Path::new(&cwd)).unwrap_or_default();
                    let shell = std::env::var("SHELL")
                        .ok()
                        .and_then(|s| s.rsplit('/').next().map(String::from))
                        .unwrap_or_else(|| "sh".into());
                    let prompt = kcmd_prompt(&what, &cwd, &branch, &os_name(), &shell);
                    let mut child = Command::new("claude")
                        .args(kcmd_args())
                        .current_dir(&cwd)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .map_err(|e| format!("claude: {e}"))?;
                    if let Some(mut i) = child.stdin.take() {
                        let _ = i.write_all(prompt.as_bytes());
                    }
                    let o = child.wait_with_output().map_err(|e| e.to_string())?;
                    let cmds = commands(&String::from_utf8_lossy(&o.stdout));
                    if cmds.is_empty() {
                        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                    } else {
                        Ok(cmds)
                    }
                });
                cx.spawn(async move |this, cx| {
                    let r = task.await;
                    let _ = this.update(cx, |this, cx| {
                        match r {
                            Ok(v) => {
                                this.opts = v;
                                this.i = 0;
                                this.phase = KPhase::Show;
                            }
                            Err(e) => {
                                this.phase = KPhase::Fail(if e.is_empty() {
                                    "o Claude não devolveu um comando".into()
                                } else {
                                    e
                                })
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
}

impl Render for KcmdView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let kbd = |k: &str| {
            div()
                .font_family(t.mono.clone())
                .text_size(px(11.))
                .px(px(4.))
                .border_1()
                .border_color(t.line)
                .rounded(px(3.))
                .child(k.to_string())
        };
        let hint = |k: &str, v: String| {
            div()
                .flex()
                .gap(px(4.))
                .items_center()
                .child(kbd(k))
                .child(v)
        };
        let mut hints = div()
            .flex()
            .flex_wrap()
            .gap(px(12.))
            .text_size(px(12.))
            .text_color(t.muted);
        hints = if self.phase == KPhase::Show {
            hints
                .child(hint("Enter", "digita no terminal (não executa)".into()))
                .child(hint(
                    "Tab",
                    format!("outra opção {}/{}", self.i + 1, self.opts.len()),
                ))
        } else {
            hints.child(hint("Enter", "pede ao Claude".into()))
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let cwd = match self.cwd.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => self.cwd.clone(),
        };
        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .unwrap_or_default();
        hints = hints.child(hint("Esc", "cancela".into())).child(
            div()
                .min_w_0()
                .truncate()
                .font_family(t.mono.clone())
                .child(format!("cwd {cwd} · {shell} · Linux")),
        );
        div()
            .w(px(620.))
            .max_w_full()
            .bg(t.raise)
            .border_1()
            .border_color(t.accent)
            .rounded(px(8.))
            .shadow_lg()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .occlude()
            .key_context("Kcmd")
            .on_action(cx.listener(|this, _: &NextOption, _, cx| {
                if this.phase == KPhase::Show && !this.opts.is_empty() {
                    this.i = (this.i + 1) % this.opts.len();
                    cx.notify();
                }
            }))
            .child(self.input.clone())
            .when(self.phase == KPhase::Think, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .text_color(t.muted)
                        .child("pensando…")
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .child("claude -p --model haiku --tools \"\""),
                        ),
                )
            })
            .when_some(
                match &self.phase {
                    KPhase::Fail(e) => Some(e.clone()),
                    _ => None,
                },
                |d, e| d.child(div().text_color(rgb(0xff5555)).child(e)),
            )
            .when(self.phase == KPhase::Show, |d| {
                d.child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(13.))
                        .bg(rgb(0x282a36))
                        .rounded(px(5.))
                        .px(px(10.))
                        .py(px(6.))
                        .text_color(t.ok)
                        .child(self.opts.get(self.i).cloned().unwrap_or_default()),
                )
            })
            .child(hints)
    }
}

#[cfg(test)]
mod tests {
    use super::{Ctx, Ev, args, commands, hunk_at, parse, prompt};

    #[test]
    fn stream_da_pergunta() {
        let evs: Vec<Ev> = include_str!("fixtures/ask-stream.jsonl")
            .lines()
            .filter_map(parse)
            .collect();
        assert_eq!(evs[0], Ev::Tool("Read /work/repo/soma.ts".into()));
        let text: String = evs
            .iter()
            .filter_map(|e| match e {
                Ev::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.starts_with("`soma.ts` exporta"), "{text}");
        assert!(matches!(
            evs.last(),
            Some(Ev::Done { session: Some(s), cost }) if s == "3cb526fd-73cd-48cc-b7c0-6ce85636372f" && *cost > 0.
        ));
        assert!(args(Some("abc")).ends_with(&["--resume".into(), "abc".into()]));
    }

    #[test]
    fn trecho_do_bloco() {
        let f = &crate::diff::parse(
            "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -10,3 +10,4 @@\n a\n-b\n+B\n+C\n d\n",
        )[0];
        let c = Ctx::of_hunk(f, 0, "worktree de x".into()).unwrap();
        assert_eq!(c.range, "a.ts:10-13");
        assert!(prompt(&c, "Explica", true).contains("+B"));
        assert_eq!(prompt(&c, "E o teste?", false), "E o teste?");
        assert_eq!(hunk_at(f, 12), 0);
    }

    #[test]
    fn comandos_da_resposta() {
        let out = "```bash\n$ kubectl get pods -n bots | grep -v Running\n```\n1. kubectl get pods -n bots\n- kubectl get pods -n bots\n\n`ss -ltnp`\nmais uma\n";
        assert_eq!(
            commands(out),
            [
                "kubectl get pods -n bots | grep -v Running",
                "kubectl get pods -n bots",
                "ss -ltnp"
            ]
        );
    }
}
