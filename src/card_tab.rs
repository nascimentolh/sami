use std::{path::PathBuf, time::Duration};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    feature,
    form::{TextField, button, checkbox},
    jira::{self, Card},
    session::Proposal,
    theme::Theme,
};

pub enum Event {
    /// (texto para o Claude da feature, toast quando chegar)
    Ask(String, String),
    Toast(String),
}

const SUBTYPES: [&str; 3] = ["Desenvolvimento", "Subtarefa", "Qualidade e Testes"];
const KINDS: [&str; 3] = ["Story", "Defeito", "Tarefa"];

struct Row {
    on: bool,
    kind: usize,
    title: Entity<TextField>,
    sp: Entity<TextField>,
}

/// A aba Card da feature (`renderCardTab` do protótipo): o card do Jira, as sub-tasks e as propostas do plano.
pub struct CardTab {
    branch: String,
    feature_id: Option<String>,
    key: Option<String>,
    /// `None`: lendo
    card: Option<Result<Card, String>>,
    title: Entity<TextField>,
    desc: Entity<TextField>,
    /// o título e a descrição como vieram do Jira: o "Salvar" compara com eles
    synced: (String, String),
    key_field: Entity<TextField>,
    info: Option<String>,
    new_kind: usize,
    new_title: Entity<TextField>,
    project: Entity<TextField>,
    rows: Vec<Row>,
    seen: u64,
    status_open: Option<usize>,
    theme: Theme,
    _poll: Task<()>,
}

impl EventEmitter<Event> for CardTab {}

fn sp(v: f64) -> String {
    format!("{v}")
}

impl CardTab {
    pub fn new(owner: PathBuf, theme: Theme, cx: &mut Context<Self>) -> Self {
        let inv = feature::find_by_path(&owner);
        let field = |v: &str, ph: &'static str, multi: bool, mono: bool, cx: &mut Context<Self>| {
            let t = theme.clone();
            let f = cx.new(|cx| TextField::new(t, v, ph, multi, mono, cx));
            cx.observe(&f, |_, _, cx| cx.notify()).detach();
            f
        };
        let (title, desc) = (
            field("", "título", false, false, cx),
            field("", "descrição", true, false, cx),
        );
        let key_field = field("", "PRD-171500", false, true, cx);
        let new_title = field("", "título do card", false, false, cx);
        let project = field("PRD", "PRD", false, true, cx);
        // ponytail: relê o card a cada 60 s com a aba existindo; webhook do Jira não chega numa máquina local
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
            }
        });
        Self {
            branch: inv.as_ref().map_or_else(
                || crate::herdr::branch(&owner).unwrap_or_default(),
                |i| i.branch.clone(),
            ),
            key: inv.as_ref().and_then(|i| i.jira.clone()),
            feature_id: inv.map(|i| i.id),
            card: None,
            title,
            desc,
            synced: Default::default(),
            key_field,
            info: None,
            new_kind: 0,
            new_title,
            project,
            rows: Vec::new(),
            seen: 0,
            status_open: None,
            theme,
            _poll: poll,
        }
    }

    /// A key vinculada e quantas propostas esperam (o rótulo da aba).
    pub fn label(&self) -> (Option<String>, usize) {
        (self.key.clone(), self.rows.len())
    }

    /// Um plano novo do Claude troca as propostas; o mesmo plano relido não mexe no que você editou.
    pub fn offer(&mut self, plan: Option<(u64, Vec<Proposal>)>, cx: &mut Context<Self>) {
        let Some((n, subs)) = plan.filter(|(n, _)| *n > self.seen) else {
            return;
        };
        self.seen = n;
        // o plano relido (painel reaberto, app reiniciado) não repropõe o que já foi para o Jira
        let sent = self
            .feature_id
            .as_ref()
            .map_or_else(Vec::new, |id| feature::state_lines(id, "subtasks"));
        let key = self.key.clone().unwrap_or_default();
        self.rows = subs
            .into_iter()
            .filter(|p| !sent.contains(&format!("{key} {}", p.title)))
            .map(|p| {
                let kind = SUBTYPES.iter().position(|t| *t == p.kind).unwrap_or(0);
                self.row(kind, &p.title, &sp(p.sp), cx)
            })
            .collect();
        cx.notify();
    }

    /// O MR do repo abriu: a sub-task de Desenvolvimento `[repo]` vai para Pull Request.
    pub fn mr_opened(&mut self, repo: &str, iid: u64, cx: &mut Context<Self>) {
        let Some(Ok(card)) = &self.card else { return };
        let prefix = format!("[{repo}]");
        let keys: Vec<String> = card
            .subs
            .iter()
            .filter(|s| s.kind == "Desenvolvimento" && s.title.starts_with(&prefix))
            .filter(|s| s.transitions.iter().any(|t| t == "Pull Request"))
            .map(|s| s.key.clone())
            .collect();
        for k in keys {
            self.transition(k, "Pull Request".into(), format!(" (MR !{iid})"), cx);
        }
    }

    fn row(&self, kind: usize, title: &str, sp: &str, cx: &mut Context<Self>) -> Row {
        let t = self.theme.clone();
        let title =
            cx.new(|cx| TextField::new(t.clone(), title, "[sistema] o que fazer", false, true, cx));
        let sp = cx.new(|cx| TextField::new(t, sp, "SP", false, true, cx));
        Row {
            on: true,
            kind,
            title,
            sp,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else { return };
        cx.spawn(async move |this, cx| {
            let k = key.clone();
            let r = cx
                .background_executor()
                .spawn(async move { jira::card(&k) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.key.as_ref() == Some(&key) {
                    this.loaded(r, cx);
                }
            });
        })
        .detach();
    }

    fn dirty(&self, cx: &App) -> bool {
        (&self.title.read(cx).value, &self.desc.read(cx).value) != (&self.synced.0, &self.synced.1)
    }

    fn loaded(&mut self, r: Result<Card, String>, cx: &mut Context<Self>) {
        if let Ok(c) = &r
            && !self.dirty(cx)
        {
            self.synced = (c.title.clone(), c.desc.clone());
            self.title.update(cx, |f, cx| f.set(c.title.clone(), cx));
            self.desc.update(cx, |f, cx| f.set(c.desc.clone(), cx));
        }
        if r.is_ok() || !matches!(self.card, Some(Ok(_))) {
            self.card = Some(r);
        }
        cx.notify();
    }

    fn link(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(id) = self.feature_id.clone() else {
            return;
        };
        if !jira::is_key(&key) {
            self.info = Some("Formato esperado: PROJ-123".into());
            return cx.notify();
        }
        self.info = Some(format!("buscando {key} no Jira…"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let k = key.clone();
            let r = cx
                .background_executor()
                .spawn(async move {
                    let c = jira::card(&k)?;
                    feature::set_jira(&id, Some(k)).map_err(|e| e.to_string())?;
                    Ok::<_, String>(c)
                })
                .await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(c) => {
                    this.info = None;
                    this.key = Some(key.clone());
                    this.synced = Default::default();
                    this.title.update(cx, |f, cx| f.set(String::new(), cx));
                    this.desc.update(cx, |f, cx| f.set(String::new(), cx));
                    this.loaded(Ok(c), cx);
                    cx.emit(Event::Toast(format!("{key} vinculado a {}", this.branch)));
                }
                Err(e) => {
                    this.info = Some(format!("Jira: {e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let title = self.new_title.read(cx).value.trim().to_string();
        let project = self.project.read(cx).value.trim().to_uppercase();
        if title.is_empty() || project.is_empty() || self.feature_id.is_none() {
            return;
        }
        let kind = KINDS[self.new_kind];
        self.info = Some(format!("criando o card em {project}…"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { jira::create(&project, kind, &title) })
                .await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(k) => this.link(k, cx),
                Err(e) => {
                    this.info = Some(format!("Jira: {e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn unlink(&mut self, cx: &mut Context<Self>) {
        let Some(id) = &self.feature_id else { return };
        if let Err(e) = feature::set_jira(id, None) {
            return cx.emit(Event::Toast(format!("Não deu para desvincular: {e}")));
        }
        self.key = None;
        self.card = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else { return };
        let (title, desc) = (
            self.title.read(cx).value.clone(),
            self.desc.read(cx).value.clone(),
        );
        cx.spawn(async move |this, cx| {
            let (k, t, d) = (key.clone(), title.clone(), desc.clone());
            let r = cx
                .background_executor()
                .spawn(async move { jira::save(&k, &t, &d) })
                .await;
            let _ = this.update(cx, |this, cx| {
                cx.emit(Event::Toast(match r {
                    Ok(()) => {
                        this.synced = (title, desc);
                        format!("Título e descrição salvos em {key}")
                    }
                    Err(e) => format!("Não deu para salvar {key}: {e}"),
                }));
                cx.notify();
            });
        })
        .detach();
    }

    fn transition(&mut self, key: String, to: String, why: String, cx: &mut Context<Self>) {
        self.status_open = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (k, s) = (key.clone(), to.clone());
            let r = cx
                .background_executor()
                .spawn(async move { jira::transition(&k, &s) })
                .await;
            let _ = this.update(cx, |this, cx| {
                cx.emit(Event::Toast(match r {
                    Ok(()) => format!("{key} → {to}{why}"),
                    Err(e) => format!("{key} não foi para {to}: {e}"),
                }));
                this.reload(cx);
            });
        })
        .detach();
    }

    fn send(&mut self, key: &str, cx: &mut Context<Self>) {
        let go: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.on)
            .map(|r| {
                format!(
                    "{} · {} · {} SP",
                    SUBTYPES[r.kind],
                    r.title.read(cx).value.trim(),
                    r.sp.read(cx).value.trim()
                )
            })
            .collect();
        if go.is_empty() {
            return;
        }
        cx.emit(Event::Ask(
            format!("/subtasks {key} Lista aprovada: {}", go.join(" | ")),
            format!(
                "/subtasks {key} enviado ao Claude de {} com {} sub-tasks",
                self.branch,
                go.len()
            ),
        ));
        if let Some(id) = &self.feature_id {
            for r in self.rows.iter().filter(|r| r.on) {
                let line = format!("{key} {}", r.title.read(cx).value.trim());
                feature::add_state_line(id, "subtasks", &line);
            }
        }
        self.rows.retain(|r| !r.on);
        cx.notify();
    }

    fn card_div(&self) -> Div {
        let t = &self.theme;
        div()
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
    }

    fn head(&self) -> Div {
        div()
            .flex()
            .gap(px(8.))
            .items_center()
            .text_size(px(13.))
            .text_color(self.theme.muted)
            .font_weight(FontWeight::MEDIUM)
    }

    fn h4(&self, title: &str) -> Div {
        self.head().child(title.to_uppercase())
    }

    fn muted(&self, text: impl Into<SharedString>) -> Div {
        div()
            .text_size(px(13.))
            .text_color(self.theme.muted)
            .child(text.into())
    }

    fn unlinked(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let feature = self.feature_id.is_some();
        let off = |b: Stateful<Div>| b.when(!feature, |b| b.opacity(0.45));
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                self.card_div()
                    .child(self.h4(&format!("Nenhum card vinculado a {}", self.branch)))
                    .child(self.muted(if feature {
                        "Vincule o card da tarefa para editar, ver e abrir as sub-tasks daqui. O Maestro lê pelo MCP do Atlassian."
                    } else {
                        "Só uma feature criada pelo Maestro guarda o card vinculado."
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .items_center()
                            .child(div().w(px(160.)).child(self.key_field.clone()))
                            .child(off(button(&t, "cj-link", "Vincular", true)).when(feature, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    let k = this.key_field.read(cx).value.trim().to_uppercase();
                                    this.link(k, cx)
                                }))
                            }))
                            .when_some(self.info.clone(), |d, i| d.child(self.muted(i))),
                    ),
            )
            .child(
                self.card_div()
                    .child(self.h4("Ou criar um card"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                button(&t, "cn-type", &format!("{} ▾", KINDS[self.new_kind]), false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.new_kind = (this.new_kind + 1) % KINDS.len();
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1().min_w(px(160.)).child(self.new_title.clone()))
                            .child(div().w(px(70.)).child(self.project.clone()))
                            .child(off(button(&t, "cn-new", "Criar pelo Claude", false)).when(feature, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| this.create(cx)))
                            })),
                    )
                    .child(self.muted(
                        "O Claude cria no projeto, atribuído a você, e vincula a esta feature.",
                    )),
            )
    }

    fn header(&self, key: &str, c: Option<&Card>, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let url = c
            .filter(|c| !c.site.is_empty())
            .map(|c| format!("{}/browse/{key}", c.site.trim_end_matches('/')));
        self.head()
            .child(
                div()
                    .font_family(t.mono.clone())
                    .text_color(t.fg)
                    .child(key.to_string()),
            )
            .when_some(c, |d, c| {
                d.child(format!("· {} · {}", c.kind, c.status).to_uppercase())
            })
            .child(div().flex_1())
            .when_some(url, |d, url| {
                d.child(
                    button(&t, "cj-web", "Abrir no Jira", false).on_click(move |_, _, _| {
                        crate::abrir_no_sistema(url.as_ref());
                    }),
                )
            })
            .child(
                button(&t, "cj-unlink", "Desvincular", false)
                    .on_click(cx.listener(|this, _, _, cx| this.unlink(cx))),
            )
    }

    fn subs(&self, c: &Card, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let mut card = self.card_div().child(
            self.h4("Sub-tasks").child(div().flex_1()).child(
                div()
                    .font_family(t.mono.clone())
                    .child(c.subs.len().to_string()),
            ),
        );
        if c.subs.is_empty() {
            return card.child(div().text_color(t.muted).child("Nenhuma ainda."));
        }
        for (i, s) in c.subs.iter().enumerate() {
            let color = match s.status.as_str() {
                "Pull Request" => t.accent,
                "Concluído" | "Archived" => t.ok,
                _ => t.fg,
            };
            let open = self.status_open == Some(i);
            let mut status = div().relative().w(px(132.)).flex_none().child(
                div()
                    .id(("sst", i))
                    .px(px(6.))
                    .py(px(2.))
                    .bg(t.win)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(4.))
                    .text_color(color)
                    .truncate()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.status_open = (!open).then_some(i);
                        cx.notify();
                    }))
                    .child(format!("{} ▾", s.status)),
            );
            if open {
                let mut list = div()
                    .absolute()
                    .top(px(24.))
                    .left_0()
                    .min_w(px(160.))
                    .py(px(4.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(5.))
                    .shadow_lg();
                if s.transitions.is_empty() {
                    list = list.child(self.muted("nenhuma transição agora").px(px(8.)).py(px(4.)));
                }
                for (j, to) in s.transitions.iter().enumerate() {
                    let (key, to2) = (s.key.clone(), to.clone());
                    list = list.child(
                        div()
                            .id(("sst-to", i * 100 + j))
                            .px(px(8.))
                            .py(px(4.))
                            .cursor_pointer()
                            .hover(|x| x.bg(t.accent_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.transition(key.clone(), to2.clone(), String::new(), cx)
                            }))
                            .child(to.clone()),
                    );
                }
                status = status.child(deferred(list));
            }
            card = card.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .text_size(px(13.))
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .font_family(t.mono.clone())
                            .text_color(t.accent)
                            .child(s.key.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(s.title.clone()))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .font_family(t.mono.clone())
                            .text_color(t.muted)
                            .child(format!("{} SP", s.sp.map_or("–".into(), sp))),
                    )
                    .child(status),
            );
        }
        card.child(self.muted(
            "Quando o MR de um repo abre, a sub-task de Desenvolvimento [repo] dele vai para Pull Request sozinha.",
        ))
    }

    fn proposals(&self, key: &str, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let mut card = self.card_div().child(
            self.h4("Propostas pelo planejamento")
                .child(div().flex_1())
                .child(
                    button(&t, "cj-add", "+ sub-task", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            let r = this.row(0, "[sistema] o que fazer", "1", cx);
                            this.rows.push(r);
                            cx.notify();
                        },
                    )),
                ),
        );
        if self.rows.is_empty() {
            let (k, branch) = (key.to_string(), self.branch.clone());
            return card
                .child(self.muted("O plano da feature propõe as sub-tasks aqui; você marca quais abrir."))
                .child(
                    div().child(
                        button(&t, "cj-ask", "Pedir ao Claude para propor sub-tasks", false).on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(Event::Ask(
                                    format!(
                                        "Proponha as sub-tasks para o que planejamos, sem criar nada: responda com {}",
                                        feature::subtasks_format(&k)
                                    ),
                                    format!("Pedido enviado ao Claude de {branch}"),
                                ))
                            }),
                        ),
                    ),
                );
        }
        for (i, r) in self.rows.iter().enumerate() {
            card = card.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .when(!r.on, |d| d.opacity(0.55))
                    .child(
                        div()
                            .id(("pon", i))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rows[i].on = !this.rows[i].on;
                                cx.notify();
                            }))
                            .child(checkbox(&t, r.on)),
                    )
                    .child(
                        button(&t, ("pty", i), SUBTYPES[r.kind], false)
                            .w(px(150.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rows[i].kind = (this.rows[i].kind + 1) % SUBTYPES.len();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().min_w_0().child(r.title.clone()))
                    .child(div().w(px(52.)).flex_none().child(r.sp.clone()))
                    .child(
                        div()
                            .id(("prm", i))
                            .px(px(4.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|x| x.text_color(t.fg))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rows.remove(i);
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            );
        }
        let n = self.rows.iter().filter(|r| r.on).count();
        let k = key.to_string();
        card.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .items_center()
                .child(
                    button(&t, "cj-open", &format!("Abrir {n} no Jira"), true)
                        .when(n == 0, |b| b.opacity(0.45))
                        .when(n > 0, |b| {
                            b.on_click(cx.listener(move |this, _, _, cx| this.send(&k, cx)))
                        }),
                )
                .child(self.muted(format!(
                    "vai ao Claude como /subtasks {key} só com as marcadas; a de testes sai com Envolvimento Testes → E2E"
                ))),
        )
    }

    fn linked(&self, key: &str, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let c = match &self.card {
            None => {
                return self
                    .card_div()
                    .child(self.header(key, None, cx))
                    .child(self.muted(format!("Lendo {key} no Jira…")));
            }
            Some(Err(e)) => {
                return self
                    .card_div()
                    .child(self.header(key, None, cx))
                    .child(div().text_color(rgb(0xff5555)).child(format!("Jira: {e}")));
            }
            Some(Ok(c)) => c.clone(),
        };
        let dirty = self.dirty(cx);
        let label = |s: &str| self.muted(s.to_string());
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                self.card_div()
                    .child(self.header(key, Some(&c), cx))
                    .child(label("Título"))
                    .child(self.title.clone())
                    .child(label("Descrição"))
                    .child(self.desc.clone())
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                button(&t, "cj-save", "Salvar no Jira", true)
                                    .when(!dirty, |b| b.opacity(0.45))
                                    .when(dirty, |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                                    }),
                            )
                            .child(self.muted(c.squad.clone())),
                    ),
            )
            .child(self.subs(&c, cx))
            .child(self.proposals(key, cx))
    }
}

impl Render for CardTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let body = match self.key.clone() {
            None => self.unlinked(cx),
            Some(k) => self.linked(&k, cx),
        };
        div()
            .id("card-tab")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .font_family(t.sans)
            .text_size(px(14.))
            .child(body)
    }
}
