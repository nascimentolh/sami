use std::{path::PathBuf, sync::Arc};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    feature::{self, Request},
    form::{FieldEvent, TextField, button, checkbox},
    herdr::Herdr,
    jira,
    sidebar::tilde,
    theme::Theme,
};

actions!(new_feature, [NextField, PrevField]);

pub enum Event {
    Done,
}

const ATTEMPTS: &[&str] = &["1", "2 (comparar no fim)", "3 (comparar no fim)"];
const MODELS: &[&str] = &["mesmo modelo", "variar entre Opus e Sonnet"];

/// O `newFeatureDlg` do protótipo.
pub struct NewFeature {
    herdr: Arc<Herdr>,
    repo: PathBuf,
    jira: Entity<TextField>,
    branch: Entity<TextField>,
    prompt: Entity<TextField>,
    bases: Vec<String>,
    base: usize,
    attempts: usize,
    model: usize,
    open: Option<&'static str>,
    copy_env: bool,
    setup: bool,
    claude: bool,
    env_files: Vec<String>,
    setup_cmd: Option<String>,
    jira_info: Option<(String, bool)>,
    err: String,
    busy: bool,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Event> for NewFeature {}

impl NewFeature {
    pub fn new(
        herdr: Arc<Herdr>,
        repo: PathBuf,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |v: &str, ph: &'static str, multi: bool, mono: bool, cx: &mut Context<Self>| {
            let t = theme.clone();
            cx.new(|cx| TextField::new(t, v, ph, multi, mono, cx))
        };
        let jira = field("", "ex.: PRD-171500", false, true, cx);
        let branch = field("feat/", "", false, true, cx);
        let prompt = field("", "ex.: adiciona paginação no GET /bots", true, false, cx);
        for f in [&jira, &branch, &prompt] {
            cx.subscribe(f, |this, f, e: &FieldEvent, cx| match e {
                FieldEvent::Changed => {
                    this.err.clear();
                    cx.notify();
                }
                FieldEvent::Submit if f == this.jira => this.lookup_jira(cx),
                FieldEvent::Submit => this.create(cx),
            })
            .detach();
        }
        let jf = jira.read(cx).focus_handle();
        cx.on_blur(&jf, window, |this, _, cx| this.lookup_jira(cx))
            .detach();
        window.focus(&jf, cx);
        Self {
            bases: feature::bases(&repo),
            env_files: feature::env_files(&repo),
            setup_cmd: feature::setup_command(&repo),
            herdr,
            repo,
            jira,
            branch,
            prompt,
            base: 0,
            attempts: 0,
            model: 0,
            open: None,
            copy_env: true,
            setup: true,
            claude: true,
            jira_info: None,
            err: String::new(),
            busy: false,
            focus: cx.focus_handle(),
            theme,
        }
    }

    fn repo_name(&self) -> String {
        self.repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn ports(&self, n: usize) -> Vec<u16> {
        let mut used: Vec<u16> = feature::inventories().iter().map(|i| i.port).collect();
        (0..n)
            .map(|_| {
                let p = feature::next_port(&used);
                used.push(p);
                p
            })
            .collect()
    }

    fn preview(&self, cx: &App) -> String {
        let b = self.branch.read(cx).value.clone();
        let n = self.attempts + 1;
        let ports = self.ports(n);
        let path = feature::worktree_path(&self.repo, if b.is_empty() { "…" } else { &b });
        format!(
            "worktree {}{} --base {} · PORT={}{} · docker compose -p {}",
            tilde(&path.to_string_lossy()),
            if n > 1 {
                format!(" (×{n}: -a, -b{})", if n > 2 { ", -c" } else { "" })
            } else {
                String::new()
            },
            self.bases.get(self.base).map_or("…", |s| s),
            ports[0],
            if n > 1 {
                format!("…{}", ports[n - 1])
            } else {
                String::new()
            },
            feature::compose_project(&self.repo, if b.is_empty() { "…" } else { &b }),
        )
    }

    fn lookup_jira(&mut self, cx: &mut Context<Self>) {
        let key = self.jira.read(cx).value.trim().to_uppercase();
        if key.is_empty() {
            self.jira_info = None;
            return cx.notify();
        }
        if !jira::is_key(&key) {
            self.jira_info = Some(("Formato esperado: PROJ-123".into(), false));
            return cx.notify();
        }
        if matches!(&self.jira_info, Some((s, _)) if s.contains(&key)) {
            return;
        }
        self.jira_info = Some((format!("buscando {key} no Jira…"), false));
        cx.notify();
        let k = key.clone();
        cx.spawn(async move |this, cx| {
            let issue = cx.background_executor().spawn(async move { jira::fetch(&k) }).await;
            let _ = this.update(cx, |this, cx| {
                match issue {
                    Ok(i) => {
                        this.jira_info = Some((format!("✓ {key} · {} · {} (MCP Atlassian)", i.kind, i.title), true));
                        let b = this.branch.read(cx).value.clone();
                        if b.is_empty() || b == "feat/" {
                            let s = feature::slug(&i.title);
                            let s = s.get(..s.len().min(28)).unwrap_or(&s).trim_end_matches('-').to_string();
                            this.branch.update(cx, |f, cx| f.set(format!("feat/{key}-{s}"), cx));
                        }
                        if this.prompt.read(cx).value.is_empty() {
                            let p = format!(
                                "Implementar {key}: {}.\nCritérios: {}.\nLeia a issue no Jira antes de começar e não commite.",
                                i.title, i.criteria
                            );
                            this.prompt.update(cx, |f, cx| f.set(p, cx));
                        }
                    }
                    Err(e) => this.jira_info = Some((format!("Jira: {e}"), false)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let br = self.branch.read(cx).value.trim().to_string();
        if br.is_empty() || br.ends_with('/') {
            self.err = "Dê um nome à branch, ex.: feat/paginacao-bots".into();
            return cx.notify();
        }
        let Some(base) = self.bases.get(self.base).cloned() else {
            self.err = "O repositório não tem branch para servir de base.".into();
            return cx.notify();
        };
        let n = self.attempts + 1;
        let models: [Option<&'static str>; 3] = if self.model == 1 {
            [Some("opus"), Some("sonnet"), Some("opus[1m]")]
        } else {
            [None; 3]
        };
        let prompt = self.prompt.read(cx).value.clone();
        let reqs: Vec<Request> = (0..n)
            .map(|i| Request {
                repo: self.repo.clone(),
                branch: if n > 1 {
                    format!("{br}-{}", ['a', 'b', 'c'][i])
                } else {
                    br.clone()
                },
                base: base.clone(),
                prompt: prompt.clone(),
                copy_env: self.copy_env,
                setup: self.setup,
                claude: self.claude,
                model: models[i],
                group: (n > 1).then(|| br.clone()),
            })
            .collect();
        let ports = self.ports(n);
        self.busy = true;
        cx.notify();
        let herdr = self.herdr.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    reqs.iter()
                        .zip(ports)
                        .try_for_each(|(r, p)| feature::create(&herdr, r, p).map(drop))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => cx.emit(Event::Done),
                    Err(e) => this.err = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cycle(&mut self, step: usize, window: &mut Window, cx: &mut Context<Self>) {
        let order = [&self.jira, &self.branch, &self.prompt].map(|f| f.read(cx).focus_handle());
        let i = order.iter().position(|f| f.is_focused(window)).unwrap_or(0);
        window.focus(&order[(i + step) % 3], cx);
        self.open = None;
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" if self.open.is_some() => self.open = None,
            "escape" => cx.emit(Event::Done),
            "enter" if k.modifiers.control => self.create(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn label(&self, text: &str) -> Div {
        div()
            .text_size(px(12.))
            .text_color(self.theme.muted)
            .child(text.to_string())
    }

    fn select(
        &self,
        id: &'static str,
        options: Vec<String>,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let open = self.open == Some(id);
        div()
            .relative()
            .child(
                div()
                    .id(id)
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(px(8.))
                    .py(px(6.))
                    .bg(t.win)
                    .border_1()
                    .border_color(if open { t.accent } else { t.line })
                    .rounded(px(5.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open = if this.open == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    }))
                    .child(options.get(selected).cloned().unwrap_or_default())
                    .child(div().text_color(t.muted).child("▾")),
            )
            .when(open, |d| {
                let mut list = div()
                    .absolute()
                    .top(px(34.))
                    .left_0()
                    .w_full()
                    .py(px(4.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(5.))
                    .shadow_lg();
                for (i, o) in options.iter().enumerate() {
                    list = list.child(
                        div()
                            .id(i)
                            .px(px(8.))
                            .py(px(4.))
                            .cursor_pointer()
                            .when(i == selected, |d| d.bg(t.accent_soft))
                            .hover(|d| d.bg(t.accent_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match id {
                                    "base" => this.base = i,
                                    "attempts" => this.attempts = i,
                                    _ => this.model = i,
                                }
                                this.open = None;
                                cx.notify();
                            }))
                            .child(o.clone()),
                    );
                }
                d.child(deferred(list))
            })
    }

    fn check(
        &self,
        id: &'static str,
        on: bool,
        label: impl IntoElement,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .gap(px(8.))
            .items_center()
            .when(enabled, |d| d.cursor_pointer())
            .when(!enabled, |d| d.opacity(0.45))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !enabled {
                    return;
                }
                match id {
                    "env" => this.copy_env = !this.copy_env,
                    "setup" => this.setup = !this.setup,
                    _ => this.claude = !this.claude,
                }
                cx.notify();
            }))
            .child(checkbox(&self.theme, on && enabled))
            .child(label)
    }
}

impl Render for NewFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let mono = |s: String| div().font_family(t.mono.clone()).child(s);
        let row2 = |a: Div, b: Div| {
            div()
                .flex()
                .gap(px(10.))
                .child(a.flex_basis(relative(0.66)))
                .child(b.flex_basis(relative(0.34)))
        };
        let fld = |label: Div, input: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(label)
                .child(input)
        };
        let env_label = if self.env_files.is_empty() {
            div()
                .flex()
                .gap(px(4.))
                .child("Copiar")
                .child(mono(".env".into()))
                .child(
                    div()
                        .text_color(t.muted)
                        .child("(nenhum ignorado pelo git)"),
                )
        } else {
            let names = self.env_files.join(", ");
            div()
                .flex()
                .gap(px(4.))
                .child("Copiar")
                .child(mono(names))
                .child("(ignorados pelo git)")
        };
        let setup_label = match &self.setup_cmd {
            Some(c) => div()
                .flex()
                .gap(px(4.))
                .child("Rodar")
                .child(mono(c.clone()))
                .child(
                    div()
                        .text_color(t.muted)
                        .child("(setup do repo, vem do .maestro.toml)"),
                ),
            None => div().flex().gap(px(4.)).child("Rodar o setup").child(
                div()
                    .text_color(t.muted)
                    .child("(sem lockfile nem .maestro.toml)"),
            ),
        };
        let bases = self.bases.clone();
        div()
            .track_focus(&self.focus)
            .key_context("NewFeature")
            .on_action(cx.listener(|this, _: &NextField, w, cx| this.cycle(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevField, w, cx| this.cycle(2, w, cx)))
            // captura: Tab e Esc chegam aqui antes dos campos e da navegação de foco do GPUI
            .capture_key_down(cx.listener(Self::on_key_down))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(13.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Nova feature em {}", self.repo_name())),
            )
            .child(fld(
                self.label("Issue do Jira (opcional: preenche branch e tarefa)"),
                self.jira.clone().into_any_element(),
            ))
            .when_some(self.jira_info.clone(), |d, (s, ok)| {
                d.child(
                    div()
                        .mt(px(-6.))
                        .text_size(px(12.))
                        .text_color(if ok { t.ok } else { t.muted })
                        .child(s),
                )
            })
            .child(row2(
                fld(self.label("Branch"), self.branch.clone().into_any_element()),
                fld(
                    self.label("A partir de"),
                    self.select("base", bases, self.base, cx).into_any_element(),
                ),
            ))
            .child(fld(
                self.label("O que o Claude deve fazer (opcional)"),
                self.prompt.clone().into_any_element(),
            ))
            .child(row2(
                fld(
                    self.label("Tentativas em paralelo"),
                    self.select(
                        "attempts",
                        ATTEMPTS.iter().map(|s| s.to_string()).collect(),
                        self.attempts,
                        cx,
                    )
                    .into_any_element(),
                ),
                fld(
                    self.label("Modelos"),
                    self.select(
                        "model",
                        MODELS.iter().map(|s| s.to_string()).collect(),
                        self.model,
                        cx,
                    )
                    .into_any_element(),
                ),
            ))
            .child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .bg(t.win)
                    .rounded(px(5.))
                    .px(px(8.))
                    .py(px(6.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(self.preview(cx)),
            )
            .child(
                div()
                    .relative()
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(6.))
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .absolute()
                            .top(px(-9.))
                            .left(px(8.))
                            .px(px(4.))
                            .bg(t.raise)
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("Ao criar"),
                    )
                    .child(self.check(
                        "env",
                        self.copy_env,
                        env_label,
                        !self.env_files.is_empty(),
                        cx,
                    ))
                    .child(self.check(
                        "setup",
                        self.setup,
                        setup_label,
                        self.setup_cmd.is_some(),
                        cx,
                    ))
                    .child(
                        self.check(
                            "claude",
                            self.claude,
                            div()
                                .flex()
                                .gap(px(4.))
                                .child("Abrir o")
                                .child(mono("claude".into()))
                                .child("nessa worktree"),
                            true,
                            cx,
                        ),
                    ),
            )
            .when(!self.err.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xff5555))
                        .child(self.err.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button(&t, "nf-no", "Cancelar", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Done))),
                    )
                    .child(
                        button(
                            &t,
                            "nf-ok",
                            if self.busy { "Criando…" } else { "Criar" },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx)))
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(11.))
                                .px(px(4.))
                                .border_1()
                                .border_color(rgb(0x282a36))
                                .rounded(px(3.))
                                .child("Ctrl+⏎"),
                        ),
                    ),
            )
    }
}
