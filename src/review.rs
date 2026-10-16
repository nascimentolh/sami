use std::{collections::HashSet, path::PathBuf, time::Duration};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    diff::{self, FileDiff, Kind},
    editor::{self, FileEditor},
    feature,
    form::{FieldEvent, TextField, button},
    mr_tab::{self, MrTab},
    theme::Theme,
    tour::{self, Anchors},
};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Review,
    Mr,
    File,
}

/// Como desfazer o que o painel acabou de fazer (o "Desfazer" do toast).
#[derive(Clone)]
pub enum Redo {
    Patch(PathBuf, String),
    Restore(PathBuf, String),
    Vault(crate::vault::Write),
    /// "Não é problema" na auditoria: desfazer tira do `audit-ignore.json`
    Audit(crate::audit::Ignored),
    /// descarte de rascunho no MR alheio
    MrDraft,
}

pub enum Event {
    Close,
    /// texto pronto para o Claude desta worktree
    Send(String),
    Toast(String, Option<Redo>),
    /// comando de testes para rodar num split do herdr
    RunTests(String),
    /// (texto para o Claude desta worktree, toast quando chegar)
    Ask(String, String),
    /// "Remover worktree" do MR mergeado
    Conclude,
    /// a aba MR abriu (o tutorial dela)
    MrOpened,
}

struct Draft {
    id: usize,
    path: String,
    line: Option<u32>,
    body: String,
    field: Option<Entity<TextField>>,
    sent: bool,
}

/// O painel "Revisar" do protótipo: o diff da feature contra a base, com aceitar, desfazer e comentar.
pub struct Review {
    pub root: PathBuf,
    branch: String,
    base: String,
    files: Option<Vec<FileDiff>>,
    accepted: HashSet<(String, String)>,
    selected: Option<(String, u32)>,
    drafts: Vec<Draft>,
    next_id: usize,
    /// edição em linha: (arquivo, linha nova, campo)
    editing: Option<(String, u32, Entity<TextField>)>,
    edited: Vec<String>,
    scroll: ScrollHandle,
    feature_id: Option<String>,
    turns: Vec<feature::Turn>,
    turn: Option<usize>,
    turn_open: bool,
    scroll_to: Option<String>,
    /// as abas de arquivo (o `S.file` do protótipo, uma por arquivo aberto) e a da frente
    editors: Vec<Entity<FileEditor>>,
    active: usize,
    /// × num arquivo com edição não salva: o segundo × descarta
    close_armed: Option<String>,
    mr: Option<Entity<MrTab>>,
    /// `S.rtab` do protótipo
    tab: Tab,
    focus: FocusHandle,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for Review {}

impl Review {
    pub fn new(
        root: PathBuf,
        branch: String,
        theme: Theme,
        anchors: Anchors,
        cx: &mut Context<Self>,
    ) -> Self {
        // ponytail: relê o diff a cada 2 s com o painel aberto; `notify` quando ficar lento em repo grande
        let poll = cx.spawn({
            let root = root.clone();
            async move |this, cx| loop {
                let r = root.clone();
                let Ok((id, turn)) = this.read_with(cx, |t, _| (t.feature_id.clone(), t.turn))
                else {
                    break;
                };
                let (d, turns) = cx
                    .background_executor()
                    .spawn(async move {
                        let turns = id.as_deref().map(feature::turns).unwrap_or_default();
                        // um turno: da foto dele até a do próximo (ou até agora)
                        let files = match turn.and_then(|n| turns.iter().position(|t| t.n == n)) {
                            Some(i) => {
                                let next = turns
                                    .get(i + 1)
                                    .map(|t| t.sha.clone())
                                    .or_else(|| feature::snapshot(&r).ok());
                                next.map(|b| {
                                    (String::from("main"), diff::between(&r, &turns[i].sha, &b))
                                })
                            }
                            None => diff::worktree(&r),
                        };
                        (files, turns)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.turns != turns {
                            this.turns = turns;
                            cx.notify();
                        }
                        let (base, files) = d.unwrap_or_default();
                        if this.files.as_ref() != Some(&files) {
                            this.base = base;
                            this.files = Some(files);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            feature_id: feature::find_by_path(&root).map(|i| i.id),
            root,
            branch,
            base: "main".into(),
            files: None,
            accepted: HashSet::new(),
            selected: None,
            drafts: Vec::new(),
            next_id: 0,
            editing: None,
            edited: Vec::new(),
            scroll: ScrollHandle::new(),
            turns: Vec::new(),
            turn: None,
            turn_open: false,
            scroll_to: None,
            editors: Vec::new(),
            active: 0,
            close_armed: None,
            mr: None,
            tab: Tab::Review,
            focus: cx.focus_handle(),
            theme,
            anchors,
            _poll: poll,
        }
    }

    fn editor(&self) -> Option<&Entity<FileEditor>> {
        self.editors.get(self.active)
    }

    /// Fecha a aba do arquivo; com edição não salva, só no segundo clique.
    fn close_file(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.editors.get(i) else { return };
        let (path, dirty) = {
            let e = e.read(cx);
            (e.path.clone(), e.dirty(cx))
        };
        if dirty && self.close_armed.as_deref() != Some(&path) {
            self.close_armed = Some(path.clone());
            cx.emit(Event::Toast(
                format!("{path} tem alterações não salvas: Ctrl+S salva, × de novo descarta"),
                None,
            ));
            return cx.notify();
        }
        self.close_armed = None;
        self.editors.remove(i);
        if self.active >= i && self.active > 0 {
            self.active -= 1;
        }
        if self.editors.is_empty() && self.tab == Tab::File {
            self.tab = Tab::Review;
        }
        let f = self.focus_handle(cx);
        window.focus(&f, cx);
        cx.notify();
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self
                .editors
                .iter()
                .any(|e| e.read(cx).focus_handle(cx).is_focused(window))
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match (self.editor(), self.tab == Tab::File) {
            (Some(e), true) => e.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        }
    }

    /// Volta para a aba do arquivo aberto, se houver.
    pub fn file_tab(&mut self, cx: &mut Context<Self>) -> bool {
        let has = !self.editors.is_empty();
        if has {
            self.tab = Tab::File;
            cx.notify();
        }
        has
    }

    pub fn show_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.tab = Tab::Review;
        self.scroll_to = Some(path);
        cx.notify();
    }

    /// Um comentário ou uma linha em edição: o Esc é dele antes do mascote.
    pub fn editing_open(&self) -> bool {
        self.editing.is_some() || self.drafts.iter().any(|d| d.field.is_some())
    }

    pub fn on_mr(&self) -> bool {
        self.tab == Tab::Mr
    }

    /// A aba MR (o `S.rtab='mr'` do protótipo); o MR é lido só a partir daqui.
    pub fn open_mr(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Mr;
        if self.mr.is_none() {
            let (root, branch, theme, anchors) = (
                self.root.clone(),
                self.branch.clone(),
                self.theme.clone(),
                self.anchors.clone(),
            );
            let m = cx.new(|cx| MrTab::new(root, branch, theme, anchors, cx));
            cx.subscribe(&m, |_, _, e: &mr_tab::Event, cx| match e {
                mr_tab::Event::Ask(t, ok) => cx.emit(Event::Ask(t.clone(), ok.clone())),
                mr_tab::Event::Conclude => cx.emit(Event::Conclude),
                mr_tab::Event::Toast(m) => cx.emit(Event::Toast(m.clone(), None)),
            })
            .detach();
            self.mr = Some(m);
        }
        cx.emit(Event::MrOpened);
        cx.notify();
    }

    /// O `openFile` do protótipo: o arquivo vira a aba ao lado de Revisar.
    pub fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::File;
        if let Some(i) = self.editors.iter().position(|e| e.read(cx).path == path) {
            self.active = i;
            let f = self.focus_handle(cx);
            window.focus(&f, cx);
            return cx.notify();
        }
        let (root, theme) = (self.root.clone(), self.theme.clone());
        let e = cx.new(|cx| FileEditor::new(root, path, theme, window, cx));
        cx.subscribe(&e, |this, _, e: &editor::Event, cx| match e {
            editor::Event::Changed => cx.notify(),
            editor::Event::Toast(m) => cx.emit(Event::Toast(m.clone(), None)),
            editor::Event::ShowDiff(p) => this.show_file(p.clone(), cx),
        })
        .detach();
        self.editors.push(e);
        self.active = self.editors.len() - 1;
        cx.notify();
    }

    /// O arquivo da aba do editor e se tem edição não salva.
    pub fn current_file(&self, cx: &App) -> Option<(String, bool, bool)> {
        let e = self.editor()?.read(cx);
        Some((e.path.clone(), e.dirty(cx), self.tab == Tab::File))
    }

    /// Linha e coluna para a barra de status, com o arquivo à frente.
    pub fn cursor(&self, cx: &App) -> Option<(usize, usize)> {
        self.editor()
            .filter(|_| self.tab == Tab::File)
            .map(|e| e.read(cx).cursor(cx))
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        match self.editor().cloned().filter(|_| self.tab == Tab::File) {
            Some(e) => e.update(cx, |e, cx| e.save(cx)),
            None => cx.emit(Event::Toast("Nada a salvar".into(), None)),
        }
    }

    /// O `sendSel` do protótipo: a seleção do editor ou a linha marcada no diff.
    pub fn selection_ref(&self, cx: &App) -> Option<String> {
        match self.editor().filter(|_| self.tab == Tab::File) {
            Some(e) => Some(e.read(cx).selection_ref(cx)),
            None => self.selected.as_ref().map(|(p, n)| format!("@{p}:{n}")),
        }
    }

    pub fn run_tests(&mut self, cx: &mut Context<Self>) {
        let files: Vec<String> = self
            .files
            .iter()
            .flatten()
            .map(|f| f.path.clone())
            .collect();
        match diff::affected_tests(&self.root, &files).filter(|_| !files.is_empty()) {
            Some(cmd) => cx.emit(Event::RunTests(cmd)),
            None => cx.emit(Event::Toast("Nenhum teste afetado para rodar".into(), None)),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.files = diff::worktree(&self.root).map(|(b, f)| {
            self.base = b;
            f
        });
        cx.notify();
    }

    /// O `rewindTo` do protótipo: os arquivos voltam para a foto de antes do turno; o toast desfaz.
    fn rewind(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(t) = self.turns.iter().find(|t| t.n == n).cloned() else {
            return;
        };
        let now = feature::snapshot(&self.root);
        match (now, feature::restore(&self.root, &t.sha)) {
            (Ok(now), Ok(())) => cx.emit(Event::Toast(
                format!("Arquivos de volta para antes do turno {n}"),
                Some(Redo::Restore(self.root.clone(), now)),
            )),
            (_, Err(e)) | (Err(e), _) => {
                cx.emit(Event::Toast(format!("Não deu para voltar: {e}"), None))
            }
        }
        self.turn = None;
        self.refresh(cx);
    }

    fn undo_hunk(&mut self, fi: usize, hi: usize, cx: &mut Context<Self>) {
        let Some(f) = self.files.as_ref().and_then(|f| f.get(fi)) else {
            return;
        };
        let patch = f.hunk_patch(hi);
        let path = f.path.clone();
        match diff::apply(&self.root, &patch, true) {
            Ok(()) => cx.emit(Event::Toast(
                format!("Bloco de {path} desfeito"),
                Some(Redo::Patch(self.root.clone(), patch)),
            )),
            Err(e) => cx.emit(Event::Toast(format!("Não deu para desfazer: {e}"), None)),
        }
        self.refresh(cx);
    }

    fn new_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.selected.clone() else {
            return;
        };
        self.next_id += 1;
        let field = self.field(self.next_id, "", window, cx);
        self.drafts.push(Draft {
            id: self.next_id,
            path,
            line: Some(line),
            body: String::new(),
            field: Some(field),
            sent: false,
        });
        cx.notify();
    }

    fn field(
        &mut self,
        id: usize,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let t = self.theme.clone();
        let f = cx
            .new(|cx| TextField::new(t, value, "O que o Claude deve mudar aqui?", true, false, cx));
        cx.subscribe(&f, move |this, f, e: &FieldEvent, cx| match e {
            FieldEvent::Submit => {
                let body = f.read(cx).value.trim().to_string();
                if let Some(d) = this.drafts.iter_mut().find(|d| d.id == id) {
                    d.body = body;
                    d.field = None;
                }
                this.drafts
                    .retain(|d| !d.body.is_empty() || d.field.is_some());
                cx.notify();
            }
            FieldEvent::Cancel => {
                this.drafts.retain(|d| d.id != id || !d.body.is_empty());
                if let Some(d) = this.drafts.iter_mut().find(|d| d.id == id) {
                    d.field = None;
                }
                cx.notify();
            }
            FieldEvent::Changed => {}
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        f
    }

    fn start_edit(
        &mut self,
        path: String,
        line: u32,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.theme.clone();
        let f = cx.new(|cx| TextField::new(t, &text, "", false, true, cx));
        cx.subscribe(&f, |this, f, e: &FieldEvent, cx| match e {
            FieldEvent::Submit => {
                let value = f.read(cx).value.clone();
                if let Some((path, line, _)) = this.editing.take() {
                    this.write_line(&path, line, &value, cx);
                }
                cx.notify();
            }
            FieldEvent::Cancel => {
                this.editing = None;
                cx.notify();
            }
            FieldEvent::Changed => {}
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.editing = Some((path, line, f));
        cx.notify();
    }

    /// Edição em linha salva direto no arquivo da worktree (é o buffer real, como no multibuffer do Zed).
    fn write_line(&mut self, path: &str, line: u32, value: &str, cx: &mut Context<Self>) {
        let file = self.root.join(path);
        let Ok(text) = std::fs::read_to_string(&file) else {
            return;
        };
        let mut lines: Vec<&str> = text.split('\n').collect();
        let i = line as usize - 1;
        if lines.get(i).is_none_or(|l| *l == value) {
            return;
        }
        lines[i] = value;
        if std::fs::write(&file, lines.join("\n")).is_ok() {
            if !self.edited.iter().any(|p| p == path) {
                self.edited.push(path.into());
            }
            cx.emit(Event::Toast(
                format!("Editado {path}:{line}. Vai junto quando enviar ao Claude."),
                None,
            ));
            self.refresh(cx);
        }
    }

    fn pending(&self) -> Vec<&Draft> {
        self.drafts
            .iter()
            .filter(|d| !d.sent && d.field.is_none() && !d.body.is_empty())
            .collect()
    }

    /// O `sendToClaude` do protótipo: comentários e edições num prompt só.
    pub fn send(&mut self, cx: &mut Context<Self>) {
        let pend = self.pending();
        if pend.is_empty() && self.edited.is_empty() {
            return;
        }
        let mut text = String::from("Revisei suas mudanças.");
        // mesmo formato do `sendToClaude` do protótipo
        if !pend.is_empty() {
            let items: Vec<String> = pend
                .iter()
                .map(|d| {
                    format!(
                        "  - {}:{}: {}",
                        d.path,
                        d.line.map_or("geral".into(), |l| l.to_string()),
                        d.body
                    )
                })
                .collect();
            text.push_str(&format!(" Ajuste:\n{}", items.join("\n")));
        }
        if !self.edited.is_empty() {
            text.push_str(&format!(
                "\n  Eu editei direto em {}; mantenha minhas edições.",
                self.edited.join(", ")
            ));
        }
        for d in self
            .drafts
            .iter_mut()
            .filter(|d| !d.sent && d.field.is_none())
        {
            d.sent = true;
        }
        self.edited.clear();
        cx.emit(Event::Send(text));
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != Tab::Review {
            return;
        }
        let k = &e.keystroke;
        match k.key.as_str() {
            "c" if !k.modifiers.modified() => self.new_draft(window, cx),
            "enter" if k.modifiers.control => self.send(cx),
            "escape" => cx.emit(Event::Close),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn turn_select(&self, n: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let label = |turn: Option<&feature::Turn>| match turn {
            None => format!(
                "Todos os turnos · {n} arquivo{}",
                if n == 1 { "" } else { "s" }
            ),
            Some(x) => {
                let p: String = x.prompt.chars().take(40).collect();
                format!(
                    "Turno {}: {p}{}",
                    x.n,
                    if x.prompt.chars().count() > 40 {
                        "…"
                    } else {
                        ""
                    }
                )
            }
        };
        let current = label(self.turn.and_then(|n| self.turns.iter().find(|t| t.n == n)));
        let mut opts: Vec<(Option<usize>, String)> = vec![(None, label(None))];
        opts.extend(self.turns.iter().map(|x| (Some(x.n), label(Some(x)))));
        div()
            .relative()
            .child(
                div()
                    .id("turn-sel")
                    .px(px(6.))
                    .py(px(2.))
                    .bg(t.win)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(5.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.turn_open = !this.turn_open;
                        cx.notify();
                    }))
                    .child(format!("{current}  ▾")),
            )
            .when(self.turn_open, |d| {
                let mut list = div()
                    .absolute()
                    .top(px(26.))
                    .left_0()
                    .min_w(px(260.))
                    .py(px(4.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(5.))
                    .shadow_lg()
                    .text_size(px(13.));
                for (i, (v, l)) in opts.into_iter().enumerate() {
                    list = list.child(
                        div()
                            .id(("turn", i))
                            .px(px(8.))
                            .py(px(4.))
                            .cursor_pointer()
                            .hover(|x| x.bg(t.accent_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.turn = v;
                                this.turn_open = false;
                                this.files = None;
                                cx.notify();
                            }))
                            .child(l),
                    );
                }
                d.child(deferred(list))
            })
    }

    fn code(&self, text: &str) -> StyledText {
        let hl: Vec<_> = diff::highlight(text)
            .into_iter()
            .map(|(r, tok)| (r, editor::style(&self.theme, tok)))
            .collect();
        StyledText::new(text.to_string()).with_highlights(hl)
    }

    fn draft_box(&self, d: &Draft, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let yellow: Hsla = rgb(0xf1fa8c).into();
        let id = d.id;
        div()
            .ml(px(94.))
            .mr(px(12.))
            .my(px(6.))
            .max_w(px(720.))
            .rounded(px(6.))
            .font_family(t.sans)
            .text_size(px(14.))
            .when(!d.sent, |x| {
                x.border_1()
                    .border_dashed()
                    .border_color(yellow)
                    .bg(Hsla { a: 0.07, ..yellow })
            })
            .when(d.sent, |x| x.border_1().border_color(t.line).opacity(0.7))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .when(!d.sent, |x| {
                        x.child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(yellow)
                                .child("PARA O CLAUDE"),
                        )
                        .child("· você")
                    })
                    .when(d.sent, |x| x.child("enviado ao Claude"))
                    .child(div().flex_1())
                    .when(!d.sent && d.field.is_none(), |x| {
                        x.child(button(t, ("ed", id), "Editar", false).on_click(cx.listener(
                            move |this, _, w, cx| {
                                let body = this
                                    .drafts
                                    .iter()
                                    .find(|d| d.id == id)
                                    .map(|d| d.body.clone())
                                    .unwrap_or_default();
                                let f = this.field(id, &body, w, cx);
                                if let Some(d) = this.drafts.iter_mut().find(|d| d.id == id) {
                                    d.field = Some(f);
                                }
                                cx.notify();
                            },
                        )))
                        .child(
                            button(t, ("del", id), "Descartar", false)
                                .text_color(rgb(0xff5555))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.drafts.retain(|d| d.id != id);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when_some(d.field.clone(), |x, f| {
                        x.child(f).child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.muted)
                                .child("Ctrl+⏎ salva · Esc cancela"),
                        )
                    })
                    .when(d.field.is_none(), |x| x.child(d.body.clone())),
            )
    }

    fn file_block(&self, fi: usize, f: &FileDiff, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let (a, r) = f.counts();
        let red: Hsla = rgb(0xff5555).into();
        let mut block = div().flex().flex_col().child(
            div()
                .flex()
                .gap(px(10.))
                .items_center()
                .px(px(12.))
                .py(px(6.))
                .bg(t.panel)
                .border_t_1()
                .border_b_1()
                .border_color(t.line)
                .font_family(t.sans)
                .child(div().font_family(t.mono.clone()).child(f.path.clone()))
                .when(f.new, |x| {
                    x.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .px(px(7.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(t.accent)
                            .text_color(t.accent)
                            .child("novo"),
                    )
                })
                .child(div().text_color(t.ok).child(format!("+{a}")))
                .child(div().text_color(red).child(format!("−{r}")))
                .when(self.edited.contains(&f.path), |x| {
                    x.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .px(px(6.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(t.accent)
                            .text_color(t.accent)
                            .child("editado por você"),
                    )
                })
                .child(div().flex_1())
                .when(self.root.join(&f.path).is_file(), |x| {
                    let path = f.path.clone();
                    x.child(button(t, ("open", fi), "Abrir no editor", false).on_click(
                        cx.listener(move |this, _, w, cx| this.open_file(path.clone(), w, cx)),
                    ))
                }),
        );
        for (hi, h) in f.hunks.iter().enumerate() {
            let ok = self.accepted.contains(&(f.path.clone(), h.header.clone()));
            let key = (f.path.clone(), h.header.clone());
            let small = |id: (&'static str, usize), label: &str| {
                div()
                    .id(id)
                    .text_size(px(12.5))
                    .px(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(4.))
                    .bg(t.raise)
                    .text_color(t.fg)
                    .cursor_pointer()
                    .hover(|x| x.border_color(t.muted))
                    .child(label.to_string())
            };
            block = block.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .text_color(if ok { t.ok } else { t.comment })
                    .bg(rgba(0x6272a41f))
                    .child(h.header.clone())
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .gap(px(4.))
                            .font_family(t.sans)
                            .when(fi == 0 && hi == 0, |x| {
                                x.relative().child(tour::anchor(&self.anchors, "hunkacts"))
                            })
                            .when(ok, |x| x.child("✓ aceito"))
                            .when(!ok, |x| {
                                x.child(small(("acc", fi * 1000 + hi), "✓ Aceitar").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.accepted.insert(key.clone());
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    small(("rej", fi * 1000 + hi), "↶ Desfazer").on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.undo_hunk(fi, hi, cx)
                                        }),
                                    ),
                                )
                            }),
                    ),
            );
            for (li, l) in h.lines.iter().enumerate() {
                let sel = self
                    .selected
                    .as_ref()
                    .is_some_and(|(p, n)| *p == f.path && Some(*n) == l.new);
                let editing = self
                    .editing
                    .as_ref()
                    .filter(|(p, n, _)| *p == f.path && Some(*n) == l.new)
                    .map(|(_, _, e)| e.clone());
                let (path, n, text) = (f.path.clone(), l.new, l.text.clone());
                let bg = match l.kind {
                    Kind::Add => Some(Hsla::from(rgba(0x50fa7b1a))),
                    Kind::Del => Some(Hsla::from(rgba(0xff55551f))),
                    Kind::Context => None,
                };
                let sign = match l.kind {
                    Kind::Add => "+",
                    Kind::Del => "-",
                    Kind::Context => "",
                };
                let num = |v: Option<u32>| {
                    div()
                        .w(px(40.))
                        .flex_none()
                        .pr(px(8.))
                        .text_right()
                        .text_color(t.comment)
                        .child(v.map(|x| x.to_string()).unwrap_or_default())
                };
                let row = div()
                    .id(("l", (fi * 1000 + hi) * 10_000 + li))
                    .group("dl")
                    .relative()
                    .flex()
                    .h(px(20.))
                    .cursor_pointer()
                    .when_some(bg, |x, c| x.bg(c))
                    .hover(|x| x.bg(rgba(0x44475a73)))
                    .when(sel, |x| x.border_l_3().border_color(t.accent))
                    .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                        let Some(n) = n else { return };
                        this.selected = Some((path.clone(), n));
                        if e.click_count() == 2 {
                            this.start_edit(path.clone(), n, text.clone(), w, cx);
                        }
                        cx.notify();
                    }))
                    .child(num(l.old))
                    .child(num(l.new))
                    .child(div().w(px(18.)).flex_none().text_color(t.muted).child(sign))
                    .child(match editing {
                        Some(f) => div().flex_1().child(f),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .pr(px(8.))
                            .child(self.code(&l.text)),
                    })
                    .when(l.new.is_some(), |x| {
                        let path = f.path.clone();
                        let n = l.new.unwrap();
                        x.child(
                            div()
                                .id(("cm", (fi * 1000 + hi) * 10_000 + li))
                                .absolute()
                                .left(px(62.))
                                .top(px(2.))
                                .size(px(16.))
                                .rounded(px(3.))
                                .bg(t.accent)
                                .text_color(rgb(0x282a36))
                                .font_family(t.sans)
                                .text_size(px(13.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .invisible()
                                .group_hover("dl", |s| s.visible())
                                .when(sel, |x| x.visible())
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.selected = Some((path.clone(), n));
                                    this.new_draft(w, cx);
                                }))
                                .child("+"),
                        )
                    });
                block = block.child(row);
                if let Some(n) = l.new {
                    for d in self
                        .drafts
                        .iter()
                        .filter(|d| d.path == f.path && d.line == Some(n))
                    {
                        block = block.child(self.draft_box(d, cx));
                    }
                }
            }
        }
        block
    }
}

impl Render for Review {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let tab = |id: &'static str, label: &str, on: bool| {
            div()
                .id(id)
                .flex()
                .gap(px(6.))
                .items_center()
                .h_full()
                .px(px(12.))
                .border_r_1()
                .border_color(t.line)
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |x| x.bg(t.win).border_b_2().border_color(t.accent))
                .child(label.to_string())
        };
        let file_tabs: Vec<_> = self
            .editors
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (name, dirty) = {
                    let e = e.read(cx);
                    let name = e.path.rsplit('/').next().unwrap_or_default().to_string();
                    (name, e.dirty(cx))
                };
                let on = self.tab == Tab::File && self.active == i;
                tab("rt-file", &name, on)
                    .id(("rt-file", i))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.tab = Tab::File;
                        this.active = i;
                        let f = this.focus_handle(cx);
                        w.focus(&f, cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("rt-x", i))
                            .w(px(14.))
                            .text_center()
                            .rounded(px(3.))
                            .text_color(if dirty { t.accent } else { t.muted })
                            .hover(|x| x.bg(t.raise).text_color(t.fg))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.close_file(i, w, cx);
                            }))
                            .child(if dirty { "●" } else { "×" }),
                    )
            })
            .collect();
        let mr_label = match self.mr.as_ref().and_then(|m| m.read(cx).fetch.clone()) {
            Some(crate::mr::Fetch::Mr(m)) => format!("MR !{}", m.iid),
            _ => "MR".into(),
        };
        let tabs = div()
            .flex()
            .items_center()
            .h(px(32.))
            .flex_none()
            .bg(t.panel)
            .border_b_1()
            .border_color(t.line)
            .child(
                tab("rt-review", "Revisar", self.tab == Tab::Review)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.tab = Tab::Review;
                        w.focus(&this.focus, cx);
                        cx.notify();
                    })),
            )
            .child(
                tab("rt-mr", &mr_label, self.tab == Tab::Mr)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.open_mr(cx))),
            )
            .child(
                div()
                    .id("rt-files")
                    .flex()
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(file_tabs),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("r-close")
                    .px(px(10.))
                    .text_color(t.muted)
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close)))
                    .child("×"),
            );
        let mr_body = self.mr.clone().filter(|_| self.tab == Tab::Mr);
        if let Some(m) = mr_body {
            return div()
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .bg(t.win)
                .text_color(t.fg)
                .font_family(t.sans)
                .text_size(px(14.))
                .child(tabs)
                .child(m);
        }
        if let Some(e) = self.editor().cloned().filter(|_| self.tab == Tab::File) {
            return div()
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .bg(t.win)
                .text_color(t.fg)
                .font_family(t.sans)
                .text_size(px(14.))
                .child(tabs)
                .child(div().flex_1().min_h_0().child(e));
        }
        let n = self.files.as_ref().map_or(0, Vec::len);
        let mut body = div()
            .id("diff")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if let Some(target) = self.scroll_to.take()
            && let Some(i) = self
                .files
                .as_ref()
                .and_then(|f| f.iter().position(|x| x.path == target))
        {
            self.scroll.scroll_to_item(i);
        }
        body = match &self.files {
            None => body.child(div().p(px(24.)).text_color(t.muted).child("Lendo o diff…")),
            Some(files) if files.is_empty() => body.child(
                div()
                    .m_auto()
                    .max_w(px(360.))
                    .p(px(24.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .text_center()
                    .text_color(t.muted)
                    .child(div().text_size(px(16.)).text_color(t.fg).child(format!("Nada para revisar em {}", self.branch)))
                    .child(format!(
                        "Quando o Claude mudar arquivos nesta worktree, o diff contra {} aparece aqui, separado por turno.",
                        self.base
                    )),
            ),
            Some(files) => {
                let files = files.clone();
                let mut list = div().flex().flex_col().pb(px(40.)).font_family(t.mono.clone()).text_size(px(13.5)).line_height(px(22.));
                for (i, f) in files.iter().enumerate() {
                    list = list.child(self.file_block(i, f, cx));
                }
                body.child(list)
            }
        };
        let pend = self.pending().len();
        let edits = self.edited.len();
        let counts = [
            (pend > 0).then(|| format!("{pend} comentário{}", if pend > 1 { "s" } else { "" })),
            (edits > 0).then(|| {
                format!(
                    "{edits} edição{} sua{}",
                    if edits > 1 { "ões" } else { "" },
                    if edits > 1 { "s" } else { "" }
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let yellow: Hsla = rgb(0xf1fa8c).into();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(tabs)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .flex_none()
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.line)
                    .relative()
                    .child(tour::anchor(&self.anchors, "rtool"))
                    .child(self.turn_select(n, cx))
                    .when_some(self.turn, |x, tn| {
                        x.child(
                            button(&t, "rew", &format!("⏪ Voltar para antes do turno {tn}"), false)
                                .on_click(cx.listener(move |this, _, _, cx| this.rewind(tn, cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child({
                        let files: Vec<String> = self.files.iter().flatten().map(|f| f.path.clone()).collect();
                        let cmd = diff::affected_tests(&self.root, &files);
                        let ready = cmd.is_some() && !files.is_empty();
                        button(&t, "aff", "▶ Testes afetados", false)
                            .when(!ready, |x| x.opacity(0.45))
                            .when_some(cmd.filter(|_| ready), |x, cmd| {
                                x.on_click(cx.listener(move |_, _, _, cx| cx.emit(Event::RunTests(cmd.clone()))))
                            })
                    })
                    .when(n > 0, |x| {
                        x.child(button(&t, "acc-all", "✓ Aceitar tudo", false).on_click(cx.listener(|this, _, _, cx| {
                            for f in this.files.iter().flatten() {
                                for h in &f.hunks {
                                    this.accepted.insert((f.path.clone(), h.header.clone()));
                                }
                            }
                            cx.emit(Event::Toast("Tudo aceito. Os blocos continuam no diff até você commitar.".into(), None));
                            cx.notify();
                        })))
                    }),
            )
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .items_center()
                    .px(px(12.))
                    .py(px(8.))
                    .flex_none()
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.line)
                    .when(counts.is_empty(), |x| {
                        x.child(div().text_color(t.muted).child("Edite as linhas aqui mesmo, aceite ou desfaça blocos, ou comente com C."))
                    })
                    .when(!counts.is_empty(), |x| {
                        x.child(div().text_color(yellow).font_weight(FontWeight::MEDIUM).child(format!("{counts} para o Claude")))
                    })
                    .child(div().flex_1())
                    .child(
                        button(&t, "d-all", "Descartar", false)
                            .text_color(rgb(0xff5555))
                            .when(pend == 0, |x| x.opacity(0.45))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.drafts.retain(|d| d.sent || d.field.is_some());
                                cx.notify();
                            })),
                    )
                    .child(
                        button(&t, "send", "Enviar ao Claude", true)
                            .relative()
                            .child(tour::anchor(&self.anchors, "send"))
                            .when(pend == 0 && edits == 0, |x| x.opacity(0.45))
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(12.))
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
