//! O modo News (`renderNews`/`renderNewsLeft` do protótipo): daily.dev filtrado pelos seus repos e pelo vault.

use std::time::Duration;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    form::{button, checkbox},
    news::{self, Cache, Profile, Scored},
    theme::{self, Theme},
    tour::{self, Anchors},
    vault,
};

pub enum Event {
    Toast(String),
    /// escrita no vault (com o Desfazer da Atividade)
    Vault(vault::Write),
    /// "Não relevante": (tag, fonte), para o Desfazer
    Hidden(String, (String, String)),
    /// a primeira leitura terminou: o shell decide se mostra o Resumo do dia
    Ready,
}

#[derive(Clone, Copy)]
struct LeftDrag;

pub struct NewsView {
    cache: Cache,
    profile: Option<Profile>,
    items: Vec<Scored>,
    ignored: Vec<(String, String)>,
    only_aff: bool,
    sel: usize,
    loading: bool,
    error: Option<String>,
    left_w: f32,
    focus: FocusHandle,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for NewsView {}

impl NewsView {
    pub fn new(theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        // ponytail: confere a cada 10 min se passou das 3 h; a coleta em si respeita o `EVERY`
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.load(false, cx)).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(600))
                    .await;
            }
        });
        Self {
            cache: Cache::load(),
            profile: None,
            items: Vec::new(),
            ignored: news::ignored(),
            only_aff: false,
            sel: 0,
            loading: false,
            error: None,
            left_w: 270.,
            focus: cx.focus_handle(),
            theme,
            anchors,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Perfil (local), coleta (no máximo a cada 3 h) e a reordenação do dia, numa thread.
    fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let mut cache = std::mem::take(&mut self.cache);
        let first = self.profile.is_none();
        cx.spawn(async move |this, cx| {
            let (cache, prof, err) = cx
                .background_executor()
                .spawn(async move {
                    let notes = vault::load(&vault::dir());
                    let prof = news::profile(&crate::mrs::repos_root(), &notes);
                    let mut err = None;
                    if news::has_token()
                        && (force || cache.stale(news::now()))
                        && let Err(e) = news::collect(&prof, &mut cache)
                    {
                        err = Some(e);
                    }
                    let today = vault::today();
                    let ranked = cache.rank.as_ref().is_some_and(|r| r.0 == today);
                    if !ranked && !cache.posts.is_empty() {
                        let xs = news::score(&cache.posts, &prof, &cache.foryou, news::now());
                        // falhou (sem claude, sem rede): fica a pontuação local até amanhã
                        let (list, tokens) = news::rerank(&prof, &xs).unwrap_or_default();
                        cache.rank = Some((today, list, tokens));
                    }
                    cache.save();
                    (cache, prof, err)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cache = cache;
                this.profile = Some(prof);
                this.error = err;
                this.loading = false;
                this.rescore();
                if first {
                    cx.emit(Event::Ready);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn rescore(&mut self) {
        let Some(prof) = &self.profile else { return };
        let rank = self
            .cache
            .rank
            .as_ref()
            .filter(|r| r.0 == vault::today())
            .map(|r| r.1.as_slice());
        let xs = news::score(&self.cache.posts, prof, &self.cache.foryou, news::now());
        self.items = news::ordered(xs, rank)
            .into_iter()
            .filter(|s| !news::is_ignored(&s.post, &self.ignored))
            .collect();
    }

    fn shown(&self) -> Vec<&Scored> {
        self.items
            .iter()
            .filter(|s| !self.only_aff || !s.aff.is_empty())
            .collect()
    }

    /// O número do botão News: o que afeta seus repos e você ainda não abriu.
    pub fn count(&self) -> usize {
        self.items
            .iter()
            .filter(|s| !s.aff.is_empty() && !self.cache.read.contains(&s.post.id))
            .count()
    }

    /// Os 5 do Resumo do dia: (título, por quê), primeiro o que afeta seus repos.
    pub fn brief(&self) -> Vec<(String, String)> {
        let mut xs: Vec<&Scored> = self.items.iter().collect();
        xs.sort_by_key(|s| s.aff.is_empty());
        xs.into_iter()
            .take(5)
            .map(|s| {
                let why = if s.aff.is_empty() {
                    s.why.clone()
                } else {
                    let more = s.aff.len().saturating_sub(2);
                    format!(
                        "pode afetar {}{}",
                        s.aff[..s.aff.len().min(2)].join(", "),
                        if more > 0 {
                            format!(" +{more}")
                        } else {
                            String::new()
                        }
                    )
                };
                (s.post.title.clone(), why)
            })
            .collect()
    }

    pub fn unhide(&mut self, rule: &(String, String), cx: &mut Context<Self>) {
        self.ignored.retain(|r| r != rule);
        news::set_ignored(&self.ignored);
        self.rescore();
        cx.notify();
    }

    fn open(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(s) = self.shown().get(i).map(|s| s.post.clone()) else {
            return;
        };
        let _ = std::process::Command::new("xdg-open").arg(&s.url).spawn();
        self.cache.read.insert(s.id);
        self.cache.save();
        cx.notify();
    }

    fn save_vault(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.shown().get(i).map(|s| s.post.clone()) else {
            return;
        };
        let date = vault::today();
        let rel = format!("daily/{date}.md");
        let before = std::fs::read_to_string(vault::dir().join(&rel)).ok();
        let text = news::with_link(before.as_deref(), &date, &p.title, &p.url);
        let short: String = p.title.chars().take(40).collect();
        match vault::write(&rel, &text, format!("{rel} · link salvo: {short}…")) {
            Ok(w) => cx.emit(Event::Vault(w)),
            Err(e) => cx.emit(Event::Toast(format!("Não deu para salvar no vault: {e}"))),
        }
    }

    fn bookmark(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(id) = self.shown().get(i).map(|s| s.post.id.clone()) else {
            return;
        };
        let on = !self.cache.bookmarks.contains(&id);
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { news::bookmark(&id, on) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        if on {
                            this.cache.bookmarks.insert(id);
                        } else {
                            this.cache.bookmarks.remove(&id);
                        }
                        this.cache.save();
                        cx.emit(Event::Toast(
                            if on {
                                "Bookmark salvo no daily.dev"
                            } else {
                                "Bookmark removido do daily.dev"
                            }
                            .into(),
                        ));
                    }
                    Err(e) => cx.emit(Event::Toast(format!("Bookmark: {e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn hide(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.shown().get(i).map(|s| s.post.clone()) else {
            return;
        };
        // o tag mais raro da coleta: #javascript esconderia meia lista
        let freq = |t: &String| {
            self.cache
                .posts
                .iter()
                .filter(|x| x.tags.contains(t))
                .count()
        };
        let tag = p
            .tags
            .iter()
            .min_by_key(|t| freq(t))
            .cloned()
            .unwrap_or_default();
        let rule = (tag, p.source.clone());
        self.ignored.push(rule.clone());
        news::set_ignored(&self.ignored);
        self.rescore();
        cx.emit(Event::Hidden(
            format!(
                "Oculto. Menos de #{} de {} daqui pra frente (só local)",
                rule.0, rule.1
            ),
            rule,
        ));
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = news::now();
        if !self.cache.stale(now) {
            let next = (self.cache.fetched_at + news::EVERY).saturating_sub(now);
            cx.emit(Event::Toast(format!(
                "Coletado {}; a próxima coleta é em {} h {:02}. A API pede no máximo 60 chamadas por minuto.",
                news::ago(self.cache.fetched_at, now),
                next / 3600,
                next % 3600 / 60
            )));
            return;
        }
        self.load(true, cx);
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt {
            return;
        }
        let n = self.shown().len();
        match k.key.as_str() {
            "j" | "down" if n > 0 => self.sel = (self.sel + 1).min(n - 1),
            "k" | "up" => self.sel = self.sel.saturating_sub(1),
            "enter" => self.open(self.sel, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn sep(&self, label: &str, n: Option<usize>) -> Div {
        let t = &self.theme;
        div()
            .flex()
            .gap(px(6.))
            .px(px(14.))
            .pt(px(8.))
            .pb(px(2.))
            .text_size(px(11.))
            .text_color(t.muted)
            .child(label.to_uppercase())
            .when_some(n, |d, n| {
                d.child(div().font_family(t.mono.clone()).child(n.to_string()))
            })
    }

    fn prof_row(&self, a: String, b: String, mono: bool) -> Div {
        let t = &self.theme;
        div()
            .flex()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(mono, |d| d.font_family(t.mono.clone()))
                    .child(a),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(t.muted)
                    .max_w(px(160.))
                    .truncate()
                    .child(b),
            )
    }

    fn left(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let prof = self.profile.clone().unwrap_or_default();
        let plural = |n: usize| format!("{n} repo{}", if n == 1 { "" } else { "s" });
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(12.))
            .pt(px(4.))
            .pb(px(10.))
            .text_size(px(12.));
        for d in prof.deps.iter().take(8) {
            rows = rows.child(self.prof_row(d.label.clone(), plural(d.repos.len()), true));
        }
        if self.profile.is_none() {
            rows = rows.child(div().text_color(t.muted).child("Lendo os repos…"));
        }
        let repos: Vec<&str> = prof.repo_notes.iter().take(3).map(String::as_str).collect();
        let vault_rows = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(12.))
            .pt(px(4.))
            .pb(px(10.))
            .text_size(px(12.))
            .child(self.prof_row(
                "decisions/".into(),
                format!("{} notas", prof.decisions.len()),
                false,
            ))
            .child(self.prof_row(
                "repos/".into(),
                if repos.is_empty() {
                    "nenhuma".into()
                } else {
                    format!("{}…", repos.join(", "))
                },
                false,
            ));
        let mut hidden = div().flex().flex_col().pt(px(4.)).pb(px(10.));
        if self.ignored.is_empty() {
            hidden = hidden.child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child("Nada oculto. “Não relevante” guarda o tag e a fonte só aqui, sem mexer no daily.dev."),
            );
        }
        for (i, rule) in self.ignored.iter().enumerate() {
            let rule = rule.clone();
            hidden = hidden.child(
                div()
                    .id(("nunh", i))
                    .flex()
                    .justify_between()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(3.))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raise))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(format!("{} · {}", rule.0, rule.1)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("mostrar"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.unhide(&rule, cx))),
            );
        }
        div()
            .id("news-left")
            .w(px(self.left_w))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Seu perfil"))
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("sem token do Claude"),
                    ),
            )
            .child(rows)
            .child(self.sep("Do vault", None))
            .child(vault_rows)
            .child(self.sep("Ocultos", Some(self.ignored.len())))
            .child(hidden)
            .child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(format!(
                        "Perfil lido de {} (package.json, Dockerfile, CI) e do vault. Atualiza quando eles mudam.",
                        crate::sidebar::tilde(&crate::mrs::repos_root().to_string_lossy())
                    )),
            )
    }

    fn empty(&self, title: &str, text: String) -> Div {
        let t = &self.theme;
        div()
            .m_auto()
            .max_w(px(460.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_color(t.muted)
            .child(
                div()
                    .text_center()
                    .text_size(px(16.))
                    .text_color(t.fg)
                    .child(title.to_string()),
            )
            .child(text)
    }

    fn no_token(&self) -> Div {
        let t = &self.theme;
        let mono = |s: &str| {
            div()
                .font_family(t.mono.clone())
                .text_color(t.fg)
                .child(s.to_string())
        };
        // os espaços vão no texto: um `gap` poria espaço antes da vírgula
        let line = |parts: &[(&str, bool)]| {
            parts.iter().fold(div().flex().flex_wrap(), |d, (s, m)| {
                d.child(if *m {
                    mono(s)
                } else {
                    div().child(s.to_string())
                })
            })
        };
        div()
            .m_auto()
            .max_w(px(520.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_color(t.muted)
            .opacity(0.9)
            .child(
                div()
                    .text_center()
                    .text_size(px(16.))
                    .text_color(t.fg)
                    .child("News precisa de um token do daily.dev"),
            )
            .child(line(&[
                ("1. Em\u{a0}", false),
                ("daily.dev/settings/api", true),
                (", gere um token pessoal.", false),
            ]))
            .child(line(&[
                ("2. Salve em\u{a0}", false),
                ("~/.config/maestro/dailydev.token", true),
                ("\u{a0}com\u{a0}", false),
                ("chmod 600", true),
                (".", false),
            ]))
            .child(div().child("3. Volte aqui: o Maestro lê o arquivo sozinho."))
            .child(
                div()
                    .text_center()
                    .child("O token nunca sai da sua máquina, a não ser para a API do daily.dev."),
            )
    }

    fn item(&self, i: usize, s: &Scored, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let p = &s.post;
        let on = i == self.sel;
        let bm = self.cache.bookmarks.contains(&p.id);
        let mut meta = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .items_center()
            .text_size(px(12.))
            .text_color(t.muted)
            .child(p.source.clone());
        if p.published > 0 {
            meta = meta.child(format!("· {}", news::ago(p.published, news::now())));
        }
        if p.read_time > 0 {
            meta = meta.child(format!("· {} min", p.read_time));
        }
        for tag in p.tags.iter().take(4) {
            meta = meta.child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(11.))
                    .text_color(t.comment)
                    .child(format!("#{tag}")),
            );
        }
        let mut why = div()
            .flex()
            .flex_wrap()
            .gap_x(px(12.))
            .gap_y(px(6.))
            .text_size(px(12.5));
        if !s.aff.is_empty() {
            let more = s.aff.len().saturating_sub(3);
            why = why.child(div().text_color(t.orange).child(format!(
                "pode afetar: {}{} ({})",
                s.aff[..s.aff.len().min(3)].join(", "),
                if more > 0 {
                    format!(" +{more}")
                } else {
                    String::new()
                },
                s.dep.clone().unwrap_or_default()
            )));
        }
        if let Some(v) = &s.vault {
            why = why.child(
                div()
                    .text_color(t.accent)
                    .child(format!("relacionado no vault: [[{v}]]")),
            );
        }
        why = why.child(div().text_color(t.muted).child(s.why.clone()));
        let act = |id: &'static str, label: &str| button(&t, (id, i), label, false);
        div()
            .id(("nitem", i))
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(t.line)
            .hover(|d| {
                d.bg(Hsla {
                    a: 0.25,
                    ..rgb(0x44475a).into()
                })
            })
            .when(on, |d| {
                d.bg(t.accent_soft)
                    .border_l_2()
                    .border_color(t.accent)
                    .pl(px(14.))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sel = i;
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .id(("ntitle", i))
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .when(self.cache.read.contains(&p.id), |d| d.text_color(t.muted))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.sel = i;
                                this.open(i, cx);
                            }))
                            .child(p.title.clone()),
                    )
                    .child(meta)
                    .child(why)
                    .when(!p.summary.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(t.muted)
                                .line_clamp(2)
                                .child(p.summary.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(8.))
                    .child(
                        act("nopen", "Abrir").on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open(i, cx);
                        })),
                    )
                    .child(act("nsv", "Salvar no vault").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.save_vault(i, cx);
                        },
                    )))
                    .child(
                        act("nbm", if bm { "★ Salvo" } else { "☆ Bookmark" }).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.bookmark(i, cx);
                            }),
                        ),
                    )
                    .child(act("nx", "Não relevante").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.hide(i, cx);
                        },
                    ))),
            )
    }

    fn center(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        if !news::has_token() {
            return div().flex_1().flex().child(self.no_token());
        }
        let now = news::now();
        let ranked = self
            .cache
            .rank
            .as_ref()
            .filter(|r| r.0 == vault::today() && !r.1.is_empty());
        let mut status = vec!["daily.dev".to_string()];
        if self.loading {
            status.push("coletando…".into());
        } else if self.cache.fetched_at > 0 {
            status.push(format!(
                "atualizado {}",
                news::ago(self.cache.fetched_at, now)
            ));
        }
        status.push(match ranked {
            Some(r) => format!(
                "ordenado pelo Claude hoje (Haiku, ~{} mil tokens)",
                (r.2 as f64 / 1000.).round().max(1.)
            ),
            None => "ordenado pela pontuação local".into(),
        });
        let bar = div()
            .flex()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap_x(px(10.))
            .gap_y(px(6.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .id("n-aff")
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .child(checkbox(&t, self.only_aff))
                    .child("Só o que afeta meus repos")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.only_aff = !this.only_aff;
                        this.sel = 0;
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_size(px(12.)).text_color(rgb(0xff5555)).child(e))
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(status.join(" · ")),
            )
            .child(
                button(&t, "n-rf", "↻", false)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let xs = self.shown();
        let mut list = div()
            .id("news-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .relative()
            .child(tour::anchor(&self.anchors, "news"));
        if xs.is_empty() {
            list = list.child(
                if self.profile.is_none() || (self.loading && self.cache.posts.is_empty()) {
                    div()
                        .p(px(24.))
                        .text_color(t.muted)
                        .child("Lendo o daily.dev…")
                } else if self.only_aff {
                    self.empty(
                        "Nada que afete seus repos hoje",
                        "Desmarque o filtro para ver o resto.".into(),
                    )
                } else if let Some(e) = &self.error {
                    self.empty("Não deu para ler o daily.dev", e.clone())
                } else {
                    self.empty(
                    "Nada do seu stack por aqui",
                    "Nenhuma notícia passou do corte de relevância. ↻ coleta de novo depois de 3 h."
                        .into(),
                )
                },
            );
        }
        let items: Vec<Scored> = xs.into_iter().cloned().collect();
        for (i, s) in items.iter().enumerate() {
            list = list.child(self.item(i, s, cx));
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
    }
}

impl Render for NewsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<LeftDrag>, _, cx| {
                let (b, x) = (e.bounds, e.event.position.x);
                let max = (f32::from(b.size.width) - 300.).max(180.);
                this.left_w = f32::from(x - b.left()).clamp(180., max);
                cx.notify();
            }))
            .child(self.left(cx))
            .child(theme::resize_handle("rz-news", LeftDrag, &t))
            .child(self.center(cx))
    }
}
